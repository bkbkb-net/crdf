use std::fmt::Debug;

use crdt_graph::TwoPTwoPGraphError;
use thiserror::Error;
use uuid::Uuid;

/// Errors that can occur when performing operations on an [`RdfGraph`](crate::RdfGraph).
#[derive(Error, Debug)]
pub enum CrdfError {
    /// An error from the underlying CRDT graph.
    #[error("Graph error: {0}")]
    Graph(#[from] TwoPTwoPGraphError<Uuid>),
    /// The specified triple was not found in the graph.
    #[error("Triple not found")]
    TripleNotFound,
    /// Literals cannot be used as subjects in RDF triples.
    #[error("Invalid subject: literals cannot be subjects")]
    LiteralSubject,
    /// Predicates must be IRIs in RDF.
    #[error("Invalid predicate: predicates must be IRIs")]
    InvalidPredicate,
    /// Language tags must be non-empty per BCP47.
    #[error("Invalid language tag: language tags must not be empty (BCP47)")]
    EmptyLanguageTag,
    /// The datatype `rdf:langString` cannot be set directly; use `with_language()` instead.
    #[error(
        "Invalid datatype: rdf:langString requires a language tag; use with_language() instead"
    )]
    LangStringDatatype,
    /// Conversion to oxrdf failed due to invalid lexical forms (IRI, blank node ID, etc.).
    #[error("oxrdf conversion error: {0}")]
    OxrdfConversion(String),
    /// An I/O error occurred while reading or writing RDF files.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// An error occurred while decoding a FlatBuffers file.
    #[error("FlatBuffers decode error: {0}")]
    FlatBuffersDecode(#[from] crdt_graph::flatbuffers::DecodeError),
}
