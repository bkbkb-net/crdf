//! Typed buses: a refinement layer over the plain 1-bit ports.
//!
//! This module is the type model plus the *lowering*,
//! and nothing else — it does not touch the netlist model, the compiler,
//! the evaluators or the editor. A typed bus is purely a refinement on
//! top of the existing 1-bit ports.
//!
//! Categorically, circuits are morphisms in a PROP
//! whose objects are natural numbers = wire counts. A *typed* bus is an
//! object of a **refined** PROP; the forgetful functor `U` back to the
//! plain wire-count PROP is exactly [`Type::width`] (it forgets the type
//! tags and keeps the bit count), and [`Type::leaf_names`] is the
//! deterministic **lowering** of a typed port to the ordered list of
//! 1-bit port names the engine already understands. Because lowering is a
//! functor that keeps morphisms and only forgets tags, adding types
//! changes no semantics: the engine, the differential fuzzer and the
//! folding pass all run unchanged on the lowered bit netlist.
//!
//! There is exactly one ground type ([`Word`]) plus two aggregates
//! ([`Type::Bundle`], [`Type::Vector`]), which is enough to express every
//! fixed-point format the standard library uses (Q1.15 signed values,
//! UQ0.15 coefficients, Q4.12 accumulators, …).

/// The only ground type: a fixed-point (or integer, or single-bit) word.
///
/// The value denoted by a raw bit pattern is `raw / 2^frac`, where `raw`
/// is interpreted as a `width`-bit two's-complement integer when `signed`
/// and as an unsigned integer otherwise. Bit order is LSB-0, matching the
/// existing port-name convention (`name.0` is the least-significant bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Word {
    /// Total bit width (`>= 1`).
    pub width: u16,
    /// Two's-complement when set; unsigned otherwise.
    pub signed: bool,
    /// Number of fractional bits (`<= width`). `0` is a plain integer.
    pub frac: u16,
}

impl Word {
    pub const fn new(width: u16, signed: bool, frac: u16) -> Self {
        Self {
            width,
            signed,
            frac,
        }
    }

    /// A single bit (`Word{1, unsigned, 0}`) — the NAND axis itself.
    pub const fn bit() -> Self {
        Self::new(1, false, 0)
    }

    /// An unsigned integer word.
    pub const fn uint(width: u16) -> Self {
        Self::new(width, false, 0)
    }

    /// A signed (two's-complement) integer word.
    pub const fn sint(width: u16) -> Self {
        Self::new(width, true, 0)
    }

    /// Signed `Q(int).(frac)` where `int` counts the sign bit (so `Q1.15`
    /// is `signed_q(1, 15)`, a 16-bit signed fraction in `[-1, 1)`).
    pub const fn signed_q(int_bits: u16, frac: u16) -> Self {
        Self::new(int_bits + frac, true, frac)
    }

    /// Unsigned `UQ(int).(frac)` (so `UQ0.15` is `unsigned_q(0, 15)`, a
    /// 15-bit coefficient in `[0, 1)`).
    pub const fn unsigned_q(int_bits: u16, frac: u16) -> Self {
        Self::new(int_bits + frac, false, frac)
    }

    /// A conventional Q-format label, e.g. `Q1.15`, `UQ0.15`, `Q4.12`.
    /// The integer part includes the sign bit for signed words.
    pub fn q_format(&self) -> String {
        let prefix = if self.signed { "Q" } else { "UQ" };
        format!("{prefix}{}.{}", self.width - self.frac, self.frac)
    }

    fn validate(&self) -> Result<(), TypeError> {
        if self.width == 0 {
            return Err(TypeError::ZeroWidth);
        }
        if self.frac > self.width {
            return Err(TypeError::FracExceedsWidth {
                width: self.width,
                frac: self.frac,
            });
        }
        Ok(())
    }
}

/// A wire type: one ground [`Word`], or an aggregate of them.
///
/// `Vector` is the document's `Vec` (an indexed homogeneous array), named
/// `Vector` here to avoid clashing with [`std::vec::Vec`]. `Bundle` is a
/// named, ordered, heterogeneous record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    /// A single fixed-point/integer/bit word.
    Word(Word),
    /// A named, ordered record of fields (the document's `Bundle`).
    Bundle(Vec<(String, Type)>),
    /// `n` copies of `elem` (the document's `Vec`).
    Vector { elem: Box<Type>, len: u16 },
}

impl Type {
    /// Convenience: a ground word type.
    pub const fn word(w: Word) -> Self {
        Self::Word(w)
    }

    /// Convenience: `len` copies of `elem`.
    pub fn vector(elem: Type, len: u16) -> Self {
        Self::Vector {
            elem: Box::new(elem),
            len,
        }
    }

    /// Convenience: a bundle from `(name, type)` fields.
    pub fn bundle(fields: impl IntoIterator<Item = (impl Into<String>, Type)>) -> Self {
        Self::Bundle(
            fields
                .into_iter()
                .map(|(name, ty)| (name.into(), ty))
                .collect(),
        )
    }

    /// The total bit width — the **forgetful functor `U` to the
    /// wire-count PROP**. This is the number of 1-bit ports the type
    /// lowers to, and equals `self.leaf_names(_).len()`.
    pub fn width(&self) -> u32 {
        match self {
            Type::Word(w) => u32::from(w.width),
            Type::Bundle(fields) => fields.iter().map(|(_, ty)| ty.width()).sum(),
            Type::Vector { elem, len } => elem.width() * u32::from(*len),
        }
    }

    /// The deterministic **lowering** of a typed port named `base` to the
    /// ordered list of 1-bit port names the engine consumes:
    ///
    /// - a 1-bit `Word` lowers to the bare `base` name;
    /// - a wider `Word` lowers to `base.0 .. base.(width-1)` (LSB-0);
    /// - a `Vector` lowers each element under `base.<i>`;
    /// - a `Bundle` lowers each field under `base.<field>`.
    ///
    /// The result length always equals [`Type::width`] (the functor
    /// square commutes). For a whole module boundary use
    /// [`Type::module_leaf_names`], which enumerates a bundle's fields
    /// with no synthetic prefix.
    pub fn leaf_names(&self, base: &str) -> Vec<String> {
        let mut out = Vec::with_capacity(self.width() as usize);
        self.lower_into(base, &mut out);
        out
    }

    fn lower_into(&self, base: &str, out: &mut Vec<String>) {
        match self {
            Type::Word(w) => {
                if w.width == 1 {
                    out.push(base.to_string());
                } else {
                    for i in 0..w.width {
                        out.push(format!("{base}.{i}"));
                    }
                }
            }
            Type::Vector { elem, len } => {
                for i in 0..*len {
                    elem.lower_into(&format!("{base}.{i}"), out);
                }
            }
            Type::Bundle(fields) => {
                for (name, ty) in fields {
                    ty.lower_into(&format!("{base}.{name}"), out);
                }
            }
        }
    }

    /// Lowering of a **module boundary** described as a top-level bundle:
    /// each field is enumerated with the field name as its base (no
    /// synthetic prefix). Non-bundle types are treated as a single
    /// unnamed field and lowered under `base`.
    pub fn module_leaf_names(&self) -> Vec<String> {
        match self {
            Type::Bundle(fields) => {
                let mut out = Vec::with_capacity(self.width() as usize);
                for (name, ty) in fields {
                    ty.lower_into(name, &mut out);
                }
                out
            }
            other => other.leaf_names(""),
        }
    }

    /// Groups the lowering by **ground-word leaf**: each entry is one
    /// `Word` node with the ordered 1-bit port names it lowers to. The
    /// entries are in the same order as [`crate::connect::plan_connection`]'s
    /// per-leaf fits, so the two zip together. Concatenating the name lists
    /// reproduces [`Type::leaf_names`].
    pub fn word_leaves(&self, base: &str) -> Vec<(Word, Vec<String>)> {
        let mut out = Vec::new();
        self.word_leaves_into(base, &mut out);
        out
    }

    fn word_leaves_into(&self, base: &str, out: &mut Vec<(Word, Vec<String>)>) {
        match self {
            Type::Word(w) => {
                let names = if w.width == 1 {
                    vec![base.to_string()]
                } else {
                    (0..w.width).map(|i| format!("{base}.{i}")).collect()
                };
                out.push((*w, names));
            }
            Type::Vector { elem, len } => {
                for i in 0..*len {
                    elem.word_leaves_into(&format!("{base}.{i}"), out);
                }
            }
            Type::Bundle(fields) => {
                for (name, ty) in fields {
                    ty.word_leaves_into(&format!("{base}.{name}"), out);
                }
            }
        }
    }

    /// Validates structural well-formedness: every `Word` has
    /// `1 <= frac <= width`, every `Vector` has `len >= 1`, and every
    /// `Bundle` has unique, non-empty, dot-free field names (the dot is
    /// the lowering path separator).
    pub fn validate(&self) -> Result<(), TypeError> {
        match self {
            Type::Word(w) => w.validate(),
            Type::Vector { elem, len } => {
                if *len == 0 {
                    return Err(TypeError::EmptyVector);
                }
                elem.validate()
            }
            Type::Bundle(fields) => {
                let mut seen = std::collections::BTreeSet::new();
                for (name, ty) in fields {
                    if name.is_empty() {
                        return Err(TypeError::EmptyFieldName);
                    }
                    if name.contains('.') {
                        return Err(TypeError::DottedFieldName(name.clone()));
                    }
                    if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                        return Err(TypeError::NonIdentifierFieldName(name.clone()));
                    }
                    if !seen.insert(name.as_str()) {
                        return Err(TypeError::DuplicateField(name.clone()));
                    }
                    ty.validate()?;
                }
                Ok(())
            }
        }
    }
}

/// Why a [`Type`] is not well-formed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeError {
    /// A `Word` with `width == 0`.
    ZeroWidth,
    /// A `Word` whose fractional bits exceed its width.
    FracExceedsWidth { width: u16, frac: u16 },
    /// A `Vector` with `len == 0`.
    EmptyVector,
    /// A `Bundle` field with an empty name.
    EmptyFieldName,
    /// A `Bundle` field name containing the `.` path separator.
    DottedFieldName(String),
    /// A `Bundle` field name with characters outside `[A-Za-z0-9_]` (which
    /// the string codec and lowered port names require).
    NonIdentifierFieldName(String),
    /// Two `Bundle` fields sharing a name.
    DuplicateField(String),
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeError::ZeroWidth => write!(f, "word width must be >= 1"),
            TypeError::FracExceedsWidth { width, frac } => {
                write!(f, "fractional bits {frac} exceed word width {width}")
            }
            TypeError::EmptyVector => write!(f, "vector length must be >= 1"),
            TypeError::EmptyFieldName => write!(f, "bundle field name must be non-empty"),
            TypeError::DottedFieldName(name) => {
                write!(f, "bundle field name {name:?} must not contain '.'")
            }
            TypeError::NonIdentifierFieldName(name) => {
                write!(f, "bundle field name {name:?} must be [A-Za-z0-9_]+")
            }
            TypeError::DuplicateField(name) => write!(f, "duplicate bundle field name {name:?}"),
        }
    }
}

impl std::error::Error for TypeError {}

impl Type {
    /// Encodes the type as a compact, deterministic, round-trippable
    /// string for CRDF persistence (`circuit:busType`). Grammar:
    /// `W<width>{S|U}<frac>` (word), `[<elem>*<len>]` (vector),
    /// `{name:<t>,name:<t>}` (bundle). Examples: `W16S15` (Q1.15),
    /// `W15U15` (UQ0.15), `[W16S15*2]` (stereo), `{l:W16S15,r:W16S15}`.
    pub fn encode(&self) -> String {
        let mut out = String::new();
        self.encode_into(&mut out);
        out
    }

    fn encode_into(&self, out: &mut String) {
        use std::fmt::Write;
        match self {
            Type::Word(w) => {
                let _ = write!(
                    out,
                    "W{}{}{}",
                    w.width,
                    if w.signed { 'S' } else { 'U' },
                    w.frac
                );
            }
            Type::Vector { elem, len } => {
                out.push('[');
                elem.encode_into(out);
                let _ = write!(out, "*{len}]");
            }
            Type::Bundle(fields) => {
                out.push('{');
                for (i, (name, ty)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(name);
                    out.push(':');
                    ty.encode_into(out);
                }
                out.push('}');
            }
        }
    }

    /// Parses a type from [`Type::encode`]'s grammar, validating the
    /// result. The inverse of `encode` on well-formed types.
    pub fn decode(text: &str) -> Result<Type, TypeParseError> {
        let mut p = TypeParser {
            bytes: text.as_bytes(),
            pos: 0,
        };
        let ty = p.parse_type()?;
        if p.pos != p.bytes.len() {
            return Err(TypeParseError::Syntax(format!(
                "trailing input at byte {}",
                p.pos
            )));
        }
        ty.validate().map_err(TypeParseError::Invalid)?;
        Ok(ty)
    }
}

/// Why a [`Type::decode`] string is not a valid type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeParseError {
    /// The string does not match the grammar.
    Syntax(String),
    /// The string parsed but the type is not well-formed.
    Invalid(TypeError),
}

impl std::fmt::Display for TypeParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeParseError::Syntax(msg) => write!(f, "type syntax error: {msg}"),
            TypeParseError::Invalid(err) => write!(f, "invalid type: {err}"),
        }
    }
}

impl std::error::Error for TypeParseError {}

struct TypeParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl TypeParser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    fn expect(&mut self, c: u8) -> Result<(), TypeParseError> {
        if self.bump() == Some(c) {
            Ok(())
        } else {
            Err(TypeParseError::Syntax(format!(
                "expected {:?} at byte {}",
                c as char,
                self.pos.saturating_sub(1)
            )))
        }
    }

    fn parse_type(&mut self) -> Result<Type, TypeParseError> {
        match self.peek() {
            Some(b'W') => self.parse_word(),
            Some(b'[') => self.parse_vector(),
            Some(b'{') => self.parse_bundle(),
            other => Err(TypeParseError::Syntax(format!(
                "expected a type at byte {} (found {:?})",
                self.pos,
                other.map(|c| c as char)
            ))),
        }
    }

    fn parse_u16(&mut self) -> Result<u16, TypeParseError> {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(TypeParseError::Syntax(format!(
                "expected a number at byte {start}"
            )));
        }
        std::str::from_utf8(&self.bytes[start..self.pos])
            .expect("ascii digits")
            .parse::<u16>()
            .map_err(|e| TypeParseError::Syntax(format!("bad number at byte {start}: {e}")))
    }

    fn parse_word(&mut self) -> Result<Type, TypeParseError> {
        self.expect(b'W')?;
        let width = self.parse_u16()?;
        let signed = match self.bump() {
            Some(b'S') => true,
            Some(b'U') => false,
            other => {
                return Err(TypeParseError::Syntax(format!(
                    "expected 'S' or 'U' at byte {} (found {:?})",
                    self.pos.saturating_sub(1),
                    other.map(|c| c as char)
                )));
            }
        };
        let frac = self.parse_u16()?;
        Ok(Type::word(Word::new(width, signed, frac)))
    }

    fn parse_vector(&mut self) -> Result<Type, TypeParseError> {
        self.expect(b'[')?;
        let elem = self.parse_type()?;
        self.expect(b'*')?;
        let len = self.parse_u16()?;
        self.expect(b']')?;
        Ok(Type::vector(elem, len))
    }

    fn parse_ident(&mut self) -> Result<String, TypeParseError> {
        let start = self.pos;
        while matches!(
            self.peek(),
            Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_')
        ) {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(TypeParseError::Syntax(format!(
                "expected a field name at byte {start}"
            )));
        }
        Ok(std::str::from_utf8(&self.bytes[start..self.pos])
            .expect("ascii identifier")
            .to_string())
    }

    fn parse_bundle(&mut self) -> Result<Type, TypeParseError> {
        self.expect(b'{')?;
        let mut fields = Vec::new();
        if self.peek() == Some(b'}') {
            self.bump();
            return Ok(Type::Bundle(fields));
        }
        loop {
            let name = self.parse_ident()?;
            self.expect(b':')?;
            let ty = self.parse_type()?;
            fields.push((name, ty));
            match self.bump() {
                Some(b',') => continue,
                Some(b'}') => break,
                other => {
                    return Err(TypeParseError::Syntax(format!(
                        "expected ',' or '}}' at byte {} (found {:?})",
                        self.pos.saturating_sub(1),
                        other.map(|c| c as char)
                    )));
                }
            }
        }
        Ok(Type::Bundle(fields))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q_format_labels_match_convention() {
        assert_eq!(Word::signed_q(1, 15).q_format(), "Q1.15"); // signed value
        assert_eq!(Word::unsigned_q(0, 15).q_format(), "UQ0.15"); // coefficient
        assert_eq!(Word::signed_q(4, 12).q_format(), "Q4.12"); // accumulator
        assert_eq!(Word::uint(24).q_format(), "UQ24.0"); // counter
        assert_eq!(Word::bit().q_format(), "UQ1.0");
    }

    #[test]
    fn width_sums_across_aggregates() {
        // A mixed bundle: enable + strobe + 24-bit count + 8-bit level +
        // 8 × 16-bit ctrl + 16-bit value.
        let bundle = Type::bundle([
            ("enable", Type::word(Word::bit())),
            ("strobe", Type::word(Word::bit())),
            ("count", Type::word(Word::uint(24))),
            ("level", Type::word(Word::unsigned_q(0, 8))),
            ("ctrl", Type::vector(Type::word(Word::signed_q(1, 15)), 8)),
            ("value", Type::word(Word::signed_q(1, 15))),
        ]);
        assert_eq!(bundle.width(), 1 + 1 + 24 + 8 + 8 * 16 + 16);
    }

    #[test]
    fn forgetful_functor_square_commutes() {
        // The key invariant: lowering then counting = the width functor.
        let cases = [
            Type::word(Word::bit()),
            Type::word(Word::signed_q(1, 15)),
            Type::vector(Type::word(Word::signed_q(1, 15)), 8),
            Type::bundle([
                ("l", Type::word(Word::signed_q(1, 15))),
                ("r", Type::word(Word::signed_q(1, 15))),
            ]),
            Type::vector(
                Type::bundle([
                    ("a", Type::word(Word::uint(4))),
                    ("b", Type::word(Word::bit())),
                ]),
                3,
            ),
        ];
        for ty in cases {
            assert_eq!(ty.leaf_names("x").len() as u32, ty.width(), "{ty:?}");
        }
    }

    #[test]
    fn lowering_uses_lsb0_dotted_names() {
        // 1-bit word: bare name.
        assert_eq!(Type::word(Word::bit()).leaf_names("gate"), ["gate"]);

        // wider word: name.0 .. name.(w-1).
        assert_eq!(
            Type::word(Word::uint(4)).leaf_names("nib"),
            ["nib.0", "nib.1", "nib.2", "nib.3"]
        );

        // vector of words: name.<i>.<bit>.
        assert_eq!(
            Type::vector(Type::word(Word::uint(2)), 2).leaf_names("v"),
            ["v.0.0", "v.0.1", "v.1.0", "v.1.1"]
        );

        // vector of single bits: name.<i> (each bit is bare under its index).
        assert_eq!(
            Type::vector(Type::word(Word::bit()), 3).leaf_names("flags"),
            ["flags.0", "flags.1", "flags.2"]
        );
    }

    #[test]
    fn module_leaf_names_drop_the_synthetic_prefix() {
        let pair = Type::bundle([
            ("enable", Type::word(Word::bit())),
            ("value", Type::vector(Type::word(Word::signed_q(1, 15)), 2)),
        ]);
        let names = pair.module_leaf_names();
        assert_eq!(names[0], "enable");
        assert_eq!(names[1], "value.0.0");
        assert_eq!(names.last().unwrap(), "value.1.15");
        assert_eq!(names.len() as u32, pair.width());
    }

    #[test]
    fn validation_rejects_ill_formed_types() {
        assert_eq!(
            Type::word(Word::new(0, false, 0)).validate(),
            Err(TypeError::ZeroWidth)
        );
        assert_eq!(
            Type::word(Word::new(8, true, 9)).validate(),
            Err(TypeError::FracExceedsWidth { width: 8, frac: 9 })
        );
        assert_eq!(
            Type::vector(Type::word(Word::bit()), 0).validate(),
            Err(TypeError::EmptyVector)
        );
        assert_eq!(
            Type::bundle([("", Type::word(Word::bit()))]).validate(),
            Err(TypeError::EmptyFieldName)
        );
        assert_eq!(
            Type::bundle([("a.b", Type::word(Word::bit()))]).validate(),
            Err(TypeError::DottedFieldName("a.b".to_string()))
        );
        assert_eq!(
            Type::bundle([
                ("dup", Type::word(Word::bit())),
                ("dup", Type::word(Word::bit())),
            ])
            .validate(),
            Err(TypeError::DuplicateField("dup".to_string()))
        );
    }

    #[test]
    fn encode_produces_expected_strings() {
        assert_eq!(Type::word(Word::signed_q(1, 15)).encode(), "W16S15");
        assert_eq!(Type::word(Word::unsigned_q(0, 15)).encode(), "W15U15");
        assert_eq!(Type::word(Word::uint(24)).encode(), "W24U0");
        assert_eq!(
            Type::vector(Type::word(Word::signed_q(1, 15)), 2).encode(),
            "[W16S15*2]"
        );
        assert_eq!(
            Type::bundle([
                ("l", Type::word(Word::signed_q(1, 15))),
                ("r", Type::word(Word::signed_q(1, 15))),
            ])
            .encode(),
            "{l:W16S15,r:W16S15}"
        );
    }

    #[test]
    fn encode_decode_round_trips() {
        let cases = [
            Type::word(Word::bit()),
            Type::word(Word::signed_q(1, 15)),
            Type::word(Word::unsigned_q(0, 15)),
            Type::word(Word::signed_q(4, 12)),
            Type::vector(Type::word(Word::signed_q(1, 15)), 8),
            Type::bundle([
                ("enable", Type::word(Word::bit())),
                ("count", Type::word(Word::uint(24))),
                ("ctrl", Type::vector(Type::word(Word::signed_q(1, 15)), 8)),
                ("value", Type::word(Word::signed_q(1, 15))),
            ]),
            Type::vector(
                Type::bundle([
                    ("a", Type::word(Word::uint(4))),
                    ("nested", Type::vector(Type::word(Word::bit()), 3)),
                ]),
                2,
            ),
            Type::Bundle(vec![]), // empty bundle
        ];
        for ty in cases {
            let encoded = ty.encode();
            let decoded = Type::decode(&encoded).unwrap_or_else(|e| panic!("{encoded}: {e}"));
            assert_eq!(decoded, ty, "round trip of {encoded}");
        }
    }

    #[test]
    fn decode_rejects_malformed_strings() {
        for bad in [
            "",
            "W16",
            "W16X15",
            "WS15",
            "[W16S15]",
            "[W16S15*]",
            "{l}",
            "{l:}",
            "{l:W16S15",
            "W16S15junk",
            "{:W16S15}",
        ] {
            assert!(Type::decode(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn non_identifier_field_names_are_rejected() {
        assert_eq!(
            Type::bundle([("a-b", Type::word(Word::bit()))]).validate(),
            Err(TypeError::NonIdentifierFieldName("a-b".to_string()))
        );
    }

    #[test]
    fn well_formed_types_validate() {
        let ty = Type::bundle([
            ("enable", Type::word(Word::bit())),
            ("ctrl", Type::vector(Type::word(Word::signed_q(1, 15)), 8)),
            ("value", Type::word(Word::signed_q(1, 15))),
        ]);
        assert_eq!(ty.validate(), Ok(()));
    }
}
