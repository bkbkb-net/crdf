#![doc = include_str!("../README.md")]

pub mod flatbuffers;
mod error;
mod graph;
mod term;
mod triple;
mod types;
mod undo;

pub use error::CrdfError;
pub use graph::{AddTripleOp, RdfFileFormat, RdfGraph, RdfOperation, RemoveTripleOp};
pub use term::{
    Literal, RDF_LANG_STRING, RdfTerm, XSD_BOOLEAN, XSD_DOUBLE, XSD_FLOAT, XSD_INT, XSD_INTEGER,
    XSD_STRING,
};
pub use triple::Triple;
pub use types::{AddEdge, AddVertex};
pub use undo::UndoManager;

pub use crdt_graph::types::{RemoveEdge, RemoveVertex};
pub use uuid::Uuid;
