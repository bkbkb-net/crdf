use std::fmt;

use crate::term::RdfTerm;

/// An RDF triple (statement) consisting of subject, predicate, and object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Triple {
    pub subject: RdfTerm,
    pub predicate: String,
    pub object: RdfTerm,
}

impl Triple {
    /// Creates a new triple with the given subject, predicate IRI, and object.
    pub fn new(subject: RdfTerm, predicate: impl Into<String>, object: RdfTerm) -> Self {
        Self {
            subject,
            predicate: predicate.into(),
            object,
        }
    }

    /// Converts this triple into an `oxrdf::Triple`.
    pub fn to_oxrdf(&self) -> Result<oxrdf::Triple, crate::CrdfError> {
        let subject = self.subject.to_oxrdf_subject()?;
        let predicate = oxrdf::NamedNode::new(self.predicate.as_str())
            .map_err(|e| crate::CrdfError::OxrdfConversion(e.to_string()))?;
        let object = self.object.to_oxrdf()?;
        Ok(oxrdf::Triple::new(subject, predicate, object))
    }
}

impl TryFrom<&Triple> for oxrdf::Triple {
    type Error = crate::CrdfError;

    fn try_from(value: &Triple) -> Result<Self, Self::Error> {
        value.to_oxrdf()
    }
}

impl From<oxrdf::Triple> for Triple {
    fn from(triple: oxrdf::Triple) -> Self {
        Self {
            subject: triple.subject.into(),
            predicate: triple.predicate.as_str().to_string(),
            object: triple.object.into(),
        }
    }
}

impl<'a> From<oxrdf::TripleRef<'a>> for Triple {
    fn from(triple: oxrdf::TripleRef<'a>) -> Self {
        Self {
            subject: triple.subject.into(),
            predicate: triple.predicate.as_str().to_string(),
            object: triple.object.into(),
        }
    }
}

/// Formats the triple in N-Triples syntax: `subject <predicate> object .`
impl fmt::Display for Triple {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} <{}> {} .", self.subject, self.predicate, self.object)
    }
}
