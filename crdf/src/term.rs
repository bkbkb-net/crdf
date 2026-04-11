use std::fmt;

use crate::error::CrdfError;

/// The IRI for `rdf:langString`, the datatype of language-tagged literals.
pub const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";

/// An RDF literal value with a datatype IRI and optional language tag.
///
/// Per RDF 1.1, every literal has a datatype. Simple literals default to
/// `xsd:string`; language-tagged literals always have datatype `rdf:langString`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Literal {
    pub value: String,
    pub datatype: String,
    pub language: Option<String>,
}

impl Literal {
    /// Creates a new literal with the given lexical value and `xsd:string` datatype.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            datatype: XSD_STRING.to_string(),
            language: None,
        }
    }

    /// Sets the datatype IRI for this literal.
    ///
    /// # Errors
    /// Returns [`CrdfError::LangStringDatatype`] if `datatype` is `rdf:langString`
    /// (use [`with_language`](Self::with_language) instead).
    pub fn with_datatype(mut self, datatype: impl Into<String>) -> Result<Self, CrdfError> {
        let dt = datatype.into();
        if dt == RDF_LANG_STRING {
            return Err(CrdfError::LangStringDatatype);
        }
        self.datatype = dt;
        self.language = None;
        Ok(self)
    }

    /// Sets the language tag, automatically setting datatype to `rdf:langString`.
    ///
    /// Per RDF 1.1 §3.3, language tags must be non-empty and are normalized to lowercase.
    ///
    /// # Errors
    /// Returns [`CrdfError::EmptyLanguageTag`] if `language` is empty.
    pub fn with_language(mut self, language: impl Into<String>) -> Result<Self, CrdfError> {
        let lang = language.into();
        if lang.is_empty() {
            return Err(CrdfError::EmptyLanguageTag);
        }
        self.language = Some(lang.to_ascii_lowercase());
        self.datatype = RDF_LANG_STRING.to_string();
        Ok(self)
    }

    /// Returns the lexical value of this literal.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Returns the datatype IRI of this literal.
    pub fn datatype(&self) -> &str {
        &self.datatype
    }

    /// Returns the language tag of this literal, if it is a language-tagged string.
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// Converts this literal into an `oxrdf::Literal`.
    pub fn to_oxrdf(&self) -> Result<oxrdf::Literal, CrdfError> {
        if let Some(lang) = &self.language {
            oxrdf::Literal::new_language_tagged_literal(self.value.as_str(), lang.as_str())
                .map_err(|e| CrdfError::OxrdfConversion(e.to_string()))
        } else {
            let datatype = oxrdf::NamedNode::new(self.datatype.as_str())
                .map_err(|e| CrdfError::OxrdfConversion(e.to_string()))?;
            Ok(oxrdf::Literal::new_typed_literal(
                self.value.as_str(),
                datatype,
            ))
        }
    }
}

impl TryFrom<&Literal> for oxrdf::Literal {
    type Error = CrdfError;

    fn try_from(value: &Literal) -> Result<Self, Self::Error> {
        value.to_oxrdf()
    }
}

/// Converts an owned `oxrdf::Literal` into a crdf [`Literal`].
impl From<oxrdf::Literal> for Literal {
    fn from(lit: oxrdf::Literal) -> Self {
        if let Some(lang) = lit.language() {
            Self {
                value: lit.value().to_string(),
                language: Some(lang.to_string()),
                datatype: RDF_LANG_STRING.to_string(),
            }
        } else {
            Self {
                value: lit.value().to_string(),
                datatype: lit.datatype().as_str().to_string(),
                language: None,
            }
        }
    }
}

/// Converts an `oxrdf::LiteralRef` into a crdf [`Literal`].
impl<'a> From<oxrdf::LiteralRef<'a>> for Literal {
    fn from(lit: oxrdf::LiteralRef<'a>) -> Self {
        if let Some(lang) = lit.language() {
            Self {
                value: lit.value().to_string(),
                language: Some(lang.to_string()),
                datatype: RDF_LANG_STRING.to_string(),
            }
        } else {
            Self {
                value: lit.value().to_string(),
                datatype: lit.datatype().as_str().to_string(),
                language: None,
            }
        }
    }
}

/// Escape a literal value for N-Triples output.
fn escape_ntriples(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

/// Formats the literal in N-Triples syntax.
///
/// - Plain literals: `"value"`
/// - Language-tagged: `"value"@lang`
/// - Typed (non-`xsd:string`): `"value"^^<datatype>`
impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\"", escape_ntriples(&self.value))?;
        if let Some(ref lang) = self.language {
            write!(f, "@{lang}")?;
        } else if self.datatype != XSD_STRING {
            write!(f, "^^<{}>", self.datatype)?;
        }
        Ok(())
    }
}

/// The IRI for `xsd:boolean`.
pub const XSD_BOOLEAN: &str = "http://www.w3.org/2001/XMLSchema#boolean";
/// The IRI for `xsd:integer`.
pub const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
/// The IRI for `xsd:int`.
pub const XSD_INT: &str = "http://www.w3.org/2001/XMLSchema#int";
/// The IRI for `xsd:double`.
pub const XSD_DOUBLE: &str = "http://www.w3.org/2001/XMLSchema#double";
/// The IRI for `xsd:float`.
pub const XSD_FLOAT: &str = "http://www.w3.org/2001/XMLSchema#float";
/// The IRI for `xsd:string`, the default datatype for plain literals.
pub const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// Creates a literal with datatype `xsd:boolean`.
impl From<bool> for Literal {
    fn from(value: bool) -> Self {
        let mut lit = Literal::new(value.to_string());
        lit.datatype = XSD_BOOLEAN.to_string();
        lit
    }
}

/// Creates a literal with datatype `xsd:int`.
impl From<i32> for Literal {
    fn from(value: i32) -> Self {
        let mut lit = Literal::new(value.to_string());
        lit.datatype = XSD_INT.to_string();
        lit
    }
}

/// Creates a literal with datatype `xsd:integer`.
impl From<i64> for Literal {
    fn from(value: i64) -> Self {
        let mut lit = Literal::new(value.to_string());
        lit.datatype = XSD_INTEGER.to_string();
        lit
    }
}

/// Creates a literal with datatype `xsd:double`.
impl From<f64> for Literal {
    fn from(value: f64) -> Self {
        let mut lit = Literal::new(value.to_string());
        lit.datatype = XSD_DOUBLE.to_string();
        lit
    }
}

/// Creates a literal with datatype `xsd:float`.
impl From<f32> for Literal {
    fn from(value: f32) -> Self {
        let mut lit = Literal::new(value.to_string());
        lit.datatype = XSD_FLOAT.to_string();
        lit
    }
}

/// Creates a literal with datatype `xsd:string`.
impl From<&str> for Literal {
    fn from(value: &str) -> Self {
        Literal::new(value)
    }
}

/// Creates a literal with datatype `xsd:string`.
impl From<String> for Literal {
    fn from(value: String) -> Self {
        Literal::new(value)
    }
}

/// Wraps a [`Literal`] into an [`RdfTerm::Literal`].
impl From<Literal> for RdfTerm {
    fn from(literal: Literal) -> Self {
        Self::Literal(literal)
    }
}

/// An RDF term that can appear as a subject or object in a triple.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RdfTerm {
    /// An IRI (Internationalized Resource Identifier).
    Iri(String),
    /// A blank node with a local identifier.
    BlankNode(String),
    /// A literal value.
    Literal(Literal),
}

impl RdfTerm {
    /// Creates an IRI term.
    pub fn iri(value: impl Into<String>) -> Self {
        Self::Iri(value.into())
    }

    /// Creates a blank node term with the given local identifier.
    pub fn blank_node(id: impl Into<String>) -> Self {
        Self::BlankNode(id.into())
    }

    /// Creates a plain literal term with datatype `xsd:string`.
    pub fn literal(value: impl Into<String>) -> Self {
        Self::Literal(Literal::new(value))
    }

    /// Returns `true` if this term is an IRI.
    pub fn is_iri(&self) -> bool {
        matches!(self, Self::Iri(_))
    }

    /// Returns `true` if this term is a blank node.
    pub fn is_blank_node(&self) -> bool {
        matches!(self, Self::BlankNode(_))
    }

    /// Returns `true` if this term is a literal.
    pub fn is_literal(&self) -> bool {
        matches!(self, Self::Literal(_))
    }

    /// Returns the IRI string if this term is an IRI, or `None` otherwise.
    pub fn as_iri(&self) -> Option<&str> {
        match self {
            Self::Iri(iri) => Some(iri),
            _ => None,
        }
    }

    /// Returns the blank node identifier if this term is a blank node, or `None` otherwise.
    pub fn as_blank_node(&self) -> Option<&str> {
        match self {
            Self::BlankNode(id) => Some(id),
            _ => None,
        }
    }

    /// Returns a reference to the literal if this term is a literal, or `None` otherwise.
    pub fn as_literal(&self) -> Option<&Literal> {
        match self {
            Self::Literal(lit) => Some(lit),
            _ => None,
        }
    }

    /// Converts this term into an `oxrdf::Term`.
    pub fn to_oxrdf(&self) -> Result<oxrdf::Term, CrdfError> {
        match self {
            Self::Iri(iri) => oxrdf::NamedNode::new(iri.as_str())
                .map(Into::into)
                .map_err(|e| CrdfError::OxrdfConversion(e.to_string())),
            Self::BlankNode(id) => oxrdf::BlankNode::new(id.as_str())
                .map(Into::into)
                .map_err(|e| CrdfError::OxrdfConversion(e.to_string())),
            Self::Literal(lit) => lit.to_oxrdf().map(Into::into),
        }
    }

    /// Converts this term into an `oxrdf::NamedOrBlankNode` for RDF subjects.
    pub fn to_oxrdf_subject(&self) -> Result<oxrdf::NamedOrBlankNode, CrdfError> {
        match self {
            Self::Iri(iri) => oxrdf::NamedNode::new(iri.as_str())
                .map(Into::into)
                .map_err(|e| CrdfError::OxrdfConversion(e.to_string())),
            Self::BlankNode(id) => oxrdf::BlankNode::new(id.as_str())
                .map(Into::into)
                .map_err(|e| CrdfError::OxrdfConversion(e.to_string())),
            Self::Literal(_) => Err(CrdfError::LiteralSubject),
        }
    }
}

impl TryFrom<&RdfTerm> for oxrdf::Term {
    type Error = CrdfError;

    fn try_from(value: &RdfTerm) -> Result<Self, Self::Error> {
        value.to_oxrdf()
    }
}

impl From<oxrdf::NamedNode> for RdfTerm {
    fn from(node: oxrdf::NamedNode) -> Self {
        Self::Iri(node.as_str().to_string())
    }
}

impl<'a> From<oxrdf::NamedNodeRef<'a>> for RdfTerm {
    fn from(node: oxrdf::NamedNodeRef<'a>) -> Self {
        Self::Iri(node.as_str().to_string())
    }
}

impl From<oxrdf::BlankNode> for RdfTerm {
    fn from(node: oxrdf::BlankNode) -> Self {
        Self::BlankNode(node.as_str().to_string())
    }
}

impl<'a> From<oxrdf::BlankNodeRef<'a>> for RdfTerm {
    fn from(node: oxrdf::BlankNodeRef<'a>) -> Self {
        Self::BlankNode(node.as_str().to_string())
    }
}

impl From<oxrdf::Literal> for RdfTerm {
    fn from(lit: oxrdf::Literal) -> Self {
        Self::Literal(lit.into())
    }
}

impl From<oxrdf::Term> for RdfTerm {
    fn from(term: oxrdf::Term) -> Self {
        match term {
            oxrdf::Term::NamedNode(n) => n.into(),
            oxrdf::Term::BlankNode(b) => b.into(),
            oxrdf::Term::Literal(l) => Self::Literal(l.into()),
        }
    }
}

impl<'a> From<oxrdf::TermRef<'a>> for RdfTerm {
    fn from(term: oxrdf::TermRef<'a>) -> Self {
        match term {
            oxrdf::TermRef::NamedNode(n) => n.into(),
            oxrdf::TermRef::BlankNode(b) => b.into(),
            oxrdf::TermRef::Literal(l) => Self::Literal(l.into()),
        }
    }
}

impl From<oxrdf::NamedOrBlankNode> for RdfTerm {
    fn from(node: oxrdf::NamedOrBlankNode) -> Self {
        match node {
            oxrdf::NamedOrBlankNode::NamedNode(n) => n.into(),
            oxrdf::NamedOrBlankNode::BlankNode(b) => b.into(),
        }
    }
}

impl<'a> From<oxrdf::NamedOrBlankNodeRef<'a>> for RdfTerm {
    fn from(subject: oxrdf::NamedOrBlankNodeRef<'a>) -> Self {
        match subject {
            oxrdf::NamedOrBlankNodeRef::NamedNode(n) => Self::Iri(n.as_str().to_string()),
            oxrdf::NamedOrBlankNodeRef::BlankNode(b) => Self::BlankNode(b.as_str().to_string()),
        }
    }
}

/// Formats the term in N-Triples syntax: `<iri>`, `_:id`, or literal.
impl fmt::Display for RdfTerm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Iri(iri) => write!(f, "<{iri}>"),
            Self::BlankNode(id) => write!(f, "_:{id}"),
            Self::Literal(lit) => write!(f, "{lit}"),
        }
    }
}
