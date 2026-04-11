use crdt_graph::{TwoPTwoPAddEdge, TwoPTwoPAddVertex, TwoPTwoPId, UpdateOperation};
use uuid::Uuid;

use crate::term::RdfTerm;

pub use crdt_graph::types::{RemoveEdge, RemoveVertex};

/// A vertex-add operation representing an RDF term in the graph.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AddVertex {
    pub id: Uuid,
    pub term: RdfTerm,
}

impl TwoPTwoPId<Uuid> for AddVertex {
    fn id(&self) -> &Uuid {
        &self.id
    }
}

impl TwoPTwoPAddVertex<Uuid> for AddVertex {}

/// An edge-add operation representing an RDF predicate connecting subject to object.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AddEdge {
    pub id: Uuid,
    pub source: Uuid,
    pub target: Uuid,
    pub predicate: String,
}

impl TwoPTwoPId<Uuid> for AddEdge {
    fn id(&self) -> &Uuid {
        &self.id
    }
}

impl TwoPTwoPAddEdge<Uuid> for AddEdge {
    fn source(&self) -> &Uuid {
        &self.source
    }
    fn target(&self) -> &Uuid {
        &self.target
    }
}

/// The underlying CRDT graph type used by [`RdfGraph`](crate::RdfGraph).
pub type Graph = crdt_graph::TwoPTwoPGraph<AddVertex, RemoveVertex, AddEdge, RemoveEdge, Uuid>;

/// An update operation on the underlying CRDT graph.
pub type GraphOperation = UpdateOperation<AddVertex, RemoveVertex, AddEdge, RemoveEdge>;

impl From<AddVertex> for GraphOperation {
    fn from(v: AddVertex) -> Self {
        UpdateOperation::AddVertex(v)
    }
}

impl From<AddEdge> for GraphOperation {
    fn from(e: AddEdge) -> Self {
        UpdateOperation::AddEdge(e)
    }
}

#[allow(dead_code)]
pub(crate) fn remove_vertex_op(v: RemoveVertex) -> GraphOperation {
    UpdateOperation::RemoveVertex(v)
}

pub(crate) fn remove_edge_op(e: RemoveEdge) -> GraphOperation {
    UpdateOperation::RemoveEdge(e)
}