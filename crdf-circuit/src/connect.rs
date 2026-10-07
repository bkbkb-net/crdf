//! The typed-connection planner.
//!
//! Given a source [`Type`] and a sink [`Type`], [`plan_connection`] decides
//! whether they connect and, per ground-word leaf, *describes* the
//! fixed-point glue a value-preserving connection needs — the binary-point
//! shift, whether the result can overflow (needs saturation) and whether
//! negatives must clamp. It **describes**, it does not build gates: the
//! gate synthesis uses saturating shifts and conditional negation.
//! This module is pure and additive — it depends
//! only on [`crate::types`].
//!
//! The plan is *value-preserving* by default (connecting two typed ports
//! usually means "keep the signal"). A cheaper, meaning-changing
//! bit-for-bit relabel (a bitcast / "reinterpret") is a separate explicit
//! user choice, reported by [`bitcast_available`], not something the
//! planner substitutes silently.
//!
//! This is the knowledge a fixed-point datapath otherwise hand-codes:
//! `Q1.15` into a `Q4.12` accumulator is a right shift by 3 with no
//! overflow, and the accumulator back to `Q1.15` is a left shift by 3
//! that must saturate — both fall straight out of the frac/width
//! arithmetic below (see the tests).

use crate::types::{Type, Word};

/// The plan for driving a `to`-typed sink from a `from`-typed source: one
/// [`WordFit`] per corresponding ground-word leaf, in lowering order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionPlan {
    /// Per ground-word-leaf fit, in the two types' shared lowering order.
    pub fits: Vec<WordFit>,
}

impl ConnectionPlan {
    /// True when every leaf is bit-for-bit `Direct` — the whole bus wires
    /// with no glue at all.
    pub fn is_direct(&self) -> bool {
        self.fits.iter().all(|f| *f == WordFit::Direct)
    }

    /// The word conversions this plan requires (skips `Direct` leaves) —
    /// what the phase-②b gate builder must emit.
    pub fn conversions(&self) -> impl Iterator<Item = &WordConvert> {
        self.fits.iter().filter_map(|f| match f {
            WordFit::Convert(c) => Some(c),
            WordFit::Direct => None,
        })
    }
}

/// How one ground-word source drives one ground-word sink, preserving
/// value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordFit {
    /// Identical word type: wire bit-for-bit, no glue.
    Direct,
    /// A value-preserving fixed-point conversion needing arithmetic glue.
    Convert(WordConvert),
}

/// A described (not yet built) value-preserving fixed-point conversion.
///
/// Semantics: to send `src`'s value to `dst`, align the binary point by
/// shifting the raw bits `shift` places (positive = left = scale up, i.e.
/// `dst.frac > src.frac`), then fit into `dst`'s width and signedness. The
/// flags tell the gate builder what it must add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WordConvert {
    pub src: Word,
    pub dst: Word,
    /// `dst.frac - src.frac`: `>0` shift left (scale up), `<0` shift right
    /// (drop low bits).
    pub shift: i32,
    /// The value can exceed `dst`'s range → the builder must saturate.
    pub saturate: bool,
    /// `src` is signed but `dst` is unsigned → negatives clamp to 0.
    pub clamp_negative: bool,
    /// The shift drops low (fractional) bits → precision loss (expected,
    /// not an error; flagged for the UI / future rounding).
    pub truncates: bool,
}

/// Why two types cannot be connected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    /// A `Word` was paired with an aggregate (or vice versa).
    ShapeMismatch,
    /// Two vectors of different length.
    VectorLength { from: u16, to: u16 },
    /// Two bundles with differing field count, names, or order.
    BundleFields,
    /// The value-preserving conversion would map the source's **entire**
    /// range to zero — the binary points are so far apart that no input can
    /// survive. That is never a connection anyone means to make (a control
    /// fraction wired into an integer increment, say), so it is refused
    /// rather than silently producing a dead signal.
    Annihilates { from: Word, to: Word },
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mismatch::ShapeMismatch => write!(f, "cannot connect a word to an aggregate"),
            Mismatch::VectorLength { from, to } => {
                write!(f, "vector length mismatch: {from} vs {to}")
            }
            Mismatch::BundleFields => write!(f, "bundle fields differ in name, order, or count"),
            Mismatch::Annihilates { from, to } => write!(
                f,
                "every {} value would convert to zero in {} — the formats are \
                 too far apart to carry the signal",
                from.q_format(),
                to.q_format()
            ),
        }
    }
}

impl std::error::Error for Mismatch {}

/// Plans a value-preserving connection from `from` to `to`, or reports why
/// their shapes are incompatible.
pub fn plan_connection(from: &Type, to: &Type) -> Result<ConnectionPlan, Mismatch> {
    let mut fits = Vec::new();
    plan_into(from, to, &mut fits)?;
    Ok(ConnectionPlan { fits })
}

/// Whether a free bit-for-bit relabel (a bitcast / "reinterpret") is
/// possible — i.e. both types occupy the same number of bits. The *value*
/// generally changes; this is an explicit user choice, distinct from the
/// value-preserving [`plan_connection`].
pub fn bitcast_available(from: &Type, to: &Type) -> bool {
    from.width() == to.width()
}

fn plan_into(from: &Type, to: &Type, out: &mut Vec<WordFit>) -> Result<(), Mismatch> {
    match (from, to) {
        (Type::Word(s), Type::Word(t)) => {
            if annihilates(*s, *t) {
                return Err(Mismatch::Annihilates { from: *s, to: *t });
            }
            out.push(fit_word(*s, *t));
            Ok(())
        }
        (Type::Vector { elem: e1, len: n1 }, Type::Vector { elem: e2, len: n2 }) => {
            if n1 != n2 {
                return Err(Mismatch::VectorLength { from: *n1, to: *n2 });
            }
            for _ in 0..*n1 {
                plan_into(e1, e2, out)?;
            }
            Ok(())
        }
        (Type::Bundle(f1), Type::Bundle(f2)) => {
            if f1.len() != f2.len() || f1.iter().zip(f2).any(|((n1, _), (n2, _))| n1 != n2) {
                return Err(Mismatch::BundleFields);
            }
            for ((_, t1), (_, t2)) in f1.iter().zip(f2) {
                plan_into(t1, t2, out)?;
            }
            Ok(())
        }
        _ => Err(Mismatch::ShapeMismatch),
    }
}

/// Whether the value-preserving conversion `src → dst` maps *every*
/// representable source value to zero.
///
/// The conversion is `dst_raw = src_raw >> (src.frac - dst.frac)`, so it
/// annihilates exactly when the shift is at least as wide as the source's
/// magnitude field: the largest source magnitude has
/// `src.width - src.signed` significant bits, and shifting all of them out
/// leaves nothing. Typed connections are supposed to make nonsense
/// impossible, and "the signal is gone" is nonsense that used to
/// type-check — a control fraction (`Q1.15`) wired into a 24-bit phase
/// increment is the case that motivated this.
fn annihilates(src: Word, dst: Word) -> bool {
    if src == dst {
        return false;
    }
    let shift = i32::from(dst.frac) - i32::from(src.frac);
    let magnitude_bits = i32::from(src.width) - i32::from(src.signed);
    shift < 0 && magnitude_bits <= -shift
}

/// The value-preserving fit for one ground-word pair.
fn fit_word(src: Word, dst: Word) -> WordFit {
    if src == dst {
        return WordFit::Direct;
    }
    let shift = i32::from(dst.frac) - i32::from(src.frac);
    // Integer bits above the binary point, and the positive magnitude bits
    // (excluding the sign bit for signed words).
    let src_int = i32::from(src.width) - i32::from(src.frac);
    let dst_int = i32::from(dst.width) - i32::from(dst.frac);
    let src_pos = src_int - i32::from(src.signed);
    let dst_pos = dst_int - i32::from(dst.signed);

    let positive_overflow = src_pos > dst_pos;
    let negative_overflow = src.signed && dst.signed && src_int > dst_int;
    let clamp_negative = src.signed && !dst.signed;

    WordFit::Convert(WordConvert {
        src,
        dst,
        shift,
        saturate: positive_overflow || negative_overflow,
        clamp_negative,
        truncates: shift < 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Word;

    fn word(w: Word) -> Type {
        Type::word(w)
    }

    fn only(plan: &ConnectionPlan) -> WordFit {
        assert_eq!(plan.fits.len(), 1, "expected a single word leaf");
        plan.fits[0]
    }

    #[test]
    fn identical_words_connect_directly() {
        let q1_15 = word(Word::signed_q(1, 15));
        let plan = plan_connection(&q1_15, &q1_15).unwrap();
        assert!(plan.is_direct());
        assert_eq!(only(&plan), WordFit::Direct);
    }

    #[test]
    fn q1_15_into_a_q4_12_accumulator_is_a_right_shift_no_saturation() {
        // `in >> 3`: Q1.15 → Q4.12 accumulator, value-preserving.
        let plan =
            plan_connection(&word(Word::signed_q(1, 15)), &word(Word::signed_q(4, 12))).unwrap();
        match only(&plan) {
            WordFit::Convert(c) => {
                assert_eq!(c.shift, -3, "12 - 15 = right shift by 3");
                assert!(!c.saturate, "Q4.12 has the headroom, no overflow");
                assert!(!c.clamp_negative);
                assert!(c.truncates, "dropping 3 fractional bits");
            }
            f => panic!("expected Convert, got {f:?}"),
        }
    }

    #[test]
    fn a_q4_12_accumulator_back_to_q1_15_saturates_on_left_shift() {
        // The output stage: Q4.12 accumulator → Q1.15 must saturate.
        let plan =
            plan_connection(&word(Word::signed_q(4, 12)), &word(Word::signed_q(1, 15))).unwrap();
        match only(&plan) {
            WordFit::Convert(c) => {
                assert_eq!(c.shift, 3, "15 - 12 = left shift by 3");
                assert!(c.saturate, "Q1.15 range is smaller — can overflow");
                assert!(!c.truncates);
            }
            f => panic!("expected Convert, got {f:?}"),
        }
    }

    #[test]
    fn signed_to_unsigned_clamps_negatives() {
        // Q1.15 (signed) → UQ1.15 (unsigned, same layout): negatives clamp.
        let plan = plan_connection(
            &word(Word::signed_q(1, 15)),
            &word(Word::new(16, false, 15)),
        )
        .unwrap();
        match only(&plan) {
            WordFit::Convert(c) => {
                assert_eq!(c.shift, 0);
                assert!(c.clamp_negative);
                assert!(!c.saturate, "the magnitude range fits");
            }
            f => panic!("expected Convert, got {f:?}"),
        }
    }

    #[test]
    fn widening_sign_extend_has_no_overflow() {
        // Q1.15 → Q1.16 (wider frac, wider word): pure scale-up, fits.
        let plan =
            plan_connection(&word(Word::signed_q(1, 15)), &word(Word::signed_q(1, 16))).unwrap();
        match only(&plan) {
            WordFit::Convert(c) => {
                assert_eq!(c.shift, 1);
                assert!(!c.saturate);
                assert!(!c.truncates);
            }
            f => panic!("expected Convert, got {f:?}"),
        }
    }

    #[test]
    fn matching_aggregates_connect_leafwise() {
        let stereo = Type::vector(word(Word::signed_q(1, 15)), 2);
        let plan = plan_connection(&stereo, &stereo).unwrap();
        assert_eq!(plan.fits.len(), 2);
        assert!(plan.is_direct());

        let frame = Type::bundle([
            ("enable", word(Word::bit())),
            ("value", word(Word::signed_q(1, 15))),
        ]);
        assert!(plan_connection(&frame, &frame).unwrap().is_direct());
    }

    #[test]
    fn a_converting_bundle_reports_its_conversions() {
        let from = Type::bundle([
            ("a", word(Word::signed_q(1, 15))),
            ("b", word(Word::signed_q(1, 15))),
        ]);
        let to = Type::bundle([
            ("a", word(Word::signed_q(1, 15))), // Direct
            ("b", word(Word::signed_q(4, 12))), // Convert (shift -3)
        ]);
        let plan = plan_connection(&from, &to).unwrap();
        assert!(!plan.is_direct());
        let conversions: Vec<_> = plan.conversions().collect();
        assert_eq!(conversions.len(), 1);
        assert_eq!(conversions[0].shift, -3);
    }

    /// A conversion that could only ever produce zero is refused, not
    /// silently built — the footgun that made a wired counter sit still.
    #[test]
    fn conversions_that_wipe_out_the_signal_are_rejected() {
        let ctrl = word(Word::signed_q(1, 15)); // a controller in [-1, 1)
        let inc = word(Word::uint(24)); // a phase increment (integer)
        assert_eq!(
            plan_connection(&ctrl, &inc),
            Err(Mismatch::Annihilates {
                from: Word::signed_q(1, 15),
                to: Word::uint(24),
            }),
            "no controller value survives a 15-place right shift"
        );
        // The unsigned coefficient form is just as dead.
        assert!(matches!(
            plan_connection(&word(Word::unsigned_q(0, 15)), &inc),
            Err(Mismatch::Annihilates { .. })
        ));

        // But lossy-yet-useful conversions still pass: Q1.15 → Q4.12 drops
        // three fractional bits and keeps the signal.
        assert!(plan_connection(&ctrl, &word(Word::signed_q(4, 12))).is_ok());
        // And one that keeps a single bit is still a connection.
        assert!(plan_connection(&ctrl, &word(Word::signed_q(15, 1))).is_ok());
    }

    #[test]
    fn shape_mismatches_are_rejected() {
        let w = word(Word::signed_q(1, 15));
        let vec2 = Type::vector(word(Word::signed_q(1, 15)), 2);
        let vec3 = Type::vector(word(Word::signed_q(1, 15)), 3);

        assert_eq!(plan_connection(&w, &vec2), Err(Mismatch::ShapeMismatch));
        assert_eq!(
            plan_connection(&vec2, &vec3),
            Err(Mismatch::VectorLength { from: 2, to: 3 })
        );

        let b1 = Type::bundle([("x", word(Word::bit()))]);
        let b2 = Type::bundle([("y", word(Word::bit()))]);
        assert_eq!(plan_connection(&b1, &b2), Err(Mismatch::BundleFields));
    }

    #[test]
    fn bitcast_available_tracks_total_width() {
        // Q1.15 (16 bits) vs UQ0.15 (15 bits): value-incompatible widths,
        // but note bitcast is only offered when the bit counts match.
        assert!(bitcast_available(
            &word(Word::signed_q(1, 15)),
            &word(Word::new(16, false, 8))
        ));
        assert!(!bitcast_available(
            &word(Word::signed_q(1, 15)),
            &word(Word::unsigned_q(0, 15))
        ));
    }
}
