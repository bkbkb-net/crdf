use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use uuid::Uuid;

use crate::error::CrdfError;
use crate::term::RdfTerm;
use crate::triple::Triple;
use crate::types::{remove_edge_op, AddEdge, AddVertex, Graph, RemoveEdge, RemoveVertex};

/// A compound operation representing an RDF triple addition.
///
/// May include vertex creation for subject and/or object if they are new terms.
#[derive(Clone, Debug)]
pub struct AddTripleOp {
    pub subject_vertex: Option<AddVertex>,
    pub object_vertex: Option<AddVertex>,
    pub edge: AddEdge,
}

/// A compound operation representing an RDF triple removal.
#[derive(Clone, Debug)]
pub struct RemoveTripleOp {
    pub edge_remove: RemoveEdge,
}

/// A high-level RDF operation that can be broadcast to other replicas.
#[derive(Clone, Debug)]
pub enum RdfOperation {
    /// A triple was added to the graph.
    AddTriple(AddTripleOp),
    /// A triple was removed from the graph.
    RemoveTriple(RemoveTripleOp),
}

/// RDF file serialization formats supported by [`RdfGraph::write_rdf_file`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RdfFileFormat {
    /// N-Triples syntax (`.nt`).
    NTriples,
    /// FlatBuffers binary format (`.crdf`) preserving full CRDT operation history.
    FlatBuffers,
}

/// An RDF graph backed by a 2P2P-Graph CRDT.
///
/// Each unique RDF term (IRI, blank node, or literal) that appears as a subject
/// or object becomes a vertex in the underlying CRDT graph. Each triple becomes
/// a directed edge from the subject vertex to the object vertex, with the
/// predicate IRI stored as edge data.
pub struct RdfGraph {
    graph: Graph,
    term_to_vertex: HashMap<RdfTerm, HashSet<Uuid>>,
}

impl Default for RdfGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl RdfGraph {
    /// Creates an empty RDF graph.
    pub fn new() -> Self {
        Self {
            graph: Graph::new(),
            term_to_vertex: HashMap::new(),
        }
    }

    /// Constructs an `RdfGraph` from pre-built CRDT graph and term mapping.
    ///
    /// Used by the FlatBuffers decoder to reconstruct graphs from serialized data.
    pub(crate) fn from_raw(graph: Graph, term_to_vertex: HashMap<RdfTerm, HashSet<Uuid>>) -> Self {
        Self {
            graph,
            term_to_vertex,
        }
    }

    /// Returns a reverse map from vertex UUID to term.
    fn vertex_to_term_map(&self) -> HashMap<&Uuid, &RdfTerm> {
        self.term_to_vertex
            .iter()
            .flat_map(|(term, ids)| ids.iter().map(move |id| (id, term)))
            .collect()
    }

    /// Adds a triple to the graph (atSource).
    ///
    /// Returns the operation to broadcast to other replicas.
    ///
    /// # Errors
    /// - [`CrdfError::LiteralSubject`] if the subject is a literal.
    pub fn add_triple(
        &mut self,
        subject: RdfTerm,
        predicate: impl Into<String>,
        object: RdfTerm,
    ) -> Result<RdfOperation, CrdfError> {
        if subject.is_literal() {
            return Err(CrdfError::LiteralSubject);
        }

        let predicate = predicate.into();

        if predicate.is_empty() {
            return Err(CrdfError::InvalidPredicate);
        }

        let (subject_vertex_op, subject_id) = self.get_or_create_vertex(subject)?;
        let (object_vertex_op, object_id) = self.get_or_create_vertex(object)?;

        let edge = AddEdge {
            id: Uuid::now_v7(),
            source: subject_id,
            target: object_id,
            predicate,
        };

        self.graph.prepare(edge.clone().into())?;

        Ok(RdfOperation::AddTriple(AddTripleOp {
            subject_vertex: subject_vertex_op,
            object_vertex: object_vertex_op,
            edge,
        }))
    }

    /// Removes a triple from the graph (atSource).
    ///
    /// Returns the operation to broadcast to other replicas.
    pub fn remove_triple(
        &mut self,
        subject: &RdfTerm,
        predicate: &str,
        object: &RdfTerm,
    ) -> Result<RdfOperation, CrdfError> {
        let subject_ids = self
            .term_to_vertex
            .get(subject)
            .ok_or(CrdfError::TripleNotFound)?;
        let object_ids = self
            .term_to_vertex
            .get(object)
            .ok_or(CrdfError::TripleNotFound)?;

        let edge = self
            .graph
            .edges()
            .find(|ea| {
                subject_ids.contains(&ea.source)
                    && object_ids.contains(&ea.target)
                    && ea.predicate == predicate
            })
            .ok_or(CrdfError::TripleNotFound)?;

        let remove = RemoveEdge {
            id: Uuid::now_v7(),
            add_edge_id: edge.id,
        };

        self.graph.prepare(remove_edge_op(remove.clone()))?;

        Ok(RdfOperation::RemoveTriple(RemoveTripleOp {
            edge_remove: remove,
        }))
    }

    /// Applies an operation received from a remote replica (downstream).
    pub fn apply_downstream(&mut self, op: RdfOperation) -> Result<(), CrdfError> {
        match op {
            RdfOperation::AddTriple(add) => {
                if let Some(sv) = add.subject_vertex {
                    self.term_to_vertex
                        .entry(sv.term.clone())
                        .or_default()
                        .insert(sv.id);
                    self.graph.apply_downstream(sv.into())?;
                }
                if let Some(ov) = add.object_vertex {
                    self.term_to_vertex
                        .entry(ov.term.clone())
                        .or_default()
                        .insert(ov.id);
                    self.graph.apply_downstream(ov.into())?;
                }
                self.graph.apply_downstream(add.edge.into())?;
            }
            RdfOperation::RemoveTriple(remove) => {
                self.graph
                    .apply_downstream(remove_edge_op(remove.edge_remove))?;
            }
        }
        Ok(())
    }

    /// Returns all active triples in the graph.
    pub fn triples(&self) -> Vec<Triple> {
        let vertex_to_term = self.vertex_to_term_map();

        self.graph
            .edges()
            .filter_map(|ea| {
                let subject = vertex_to_term.get(&ea.source)?;
                let object = vertex_to_term.get(&ea.target)?;
                Some(Triple::new(
                    (*subject).clone(),
                    ea.predicate.clone(),
                    (*object).clone(),
                ))
            })
            .collect()
    }

    /// Finds triples matching a pattern. `None` means "match any".
    pub fn triples_matching(
        &self,
        subject: Option<&RdfTerm>,
        predicate: Option<&str>,
        object: Option<&RdfTerm>,
    ) -> Vec<Triple> {
        self.triples()
            .into_iter()
            .filter(|t| {
                subject.is_none_or(|s| t.subject == *s)
                    && predicate.is_none_or(|p| t.predicate == p)
                    && object.is_none_or(|o| t.object == *o)
            })
            .collect()
    }

    /// Returns `true` if the graph contains the specified triple.
    pub fn contains_triple(&self, subject: &RdfTerm, predicate: &str, object: &RdfTerm) -> bool {
        let Some(subject_ids) = self.term_to_vertex.get(subject) else {
            return false;
        };
        let Some(object_ids) = self.term_to_vertex.get(object) else {
            return false;
        };

        self.graph.edges().any(|ea| {
            subject_ids.contains(&ea.source)
                && object_ids.contains(&ea.target)
                && ea.predicate == predicate
        })
    }

    /// Returns the number of active triples in the graph.
    pub fn len(&self) -> usize {
        self.graph.edge_count()
    }

    /// Returns `true` if the graph contains no active triples.
    pub fn is_empty(&self) -> bool {
        self.graph.is_empty()
    }

    /// Returns all vertex-add operations (`V_A`) in the underlying CRDT graph.
    pub fn all_vertices_added(&self) -> &[AddVertex] {
        self.graph.all_vertices_added()
    }

    /// Returns all vertex-remove operations (`V_R`) in the underlying CRDT graph.
    pub fn all_vertices_removed(&self) -> &[RemoveVertex] {
        self.graph.all_vertices_removed()
    }

    /// Returns all edge-add operations (`E_A`) in the underlying CRDT graph.
    pub fn all_edges_added(&self) -> &[AddEdge] {
        self.graph.all_edges_added()
    }

    /// Returns all edge-remove operations (`E_R`) in the underlying CRDT graph.
    pub fn all_edges_removed(&self) -> &[RemoveEdge] {
        self.graph.all_edges_removed()
    }

    /// Returns all unique subjects that appear in active triples.
    pub fn subjects(&self) -> Vec<&RdfTerm> {
        let vertex_to_term = self.vertex_to_term_map();

        let mut seen = HashSet::new();
        self.graph
            .edges()
            .filter_map(|ea| vertex_to_term.get(&ea.source).copied())
            .filter(|term| seen.insert(*term as *const RdfTerm))
            .collect()
    }

    /// Returns all unique predicates that appear in active triples.
    pub fn predicates(&self) -> Vec<&str> {
        let mut predicates: Vec<&str> =
            self.graph.edges().map(|ea| ea.predicate.as_str()).collect();
        predicates.sort();
        predicates.dedup();
        predicates
    }

    /// Returns all unique objects that appear in active triples.
    pub fn objects(&self) -> Vec<&RdfTerm> {
        let vertex_to_term = self.vertex_to_term_map();

        let mut seen = HashSet::new();
        self.graph
            .edges()
            .filter_map(|ea| vertex_to_term.get(&ea.target).copied())
            .filter(|term| seen.insert(*term as *const RdfTerm))
            .collect()
    }

    /// Returns all triples with the given subject.
    pub fn triples_for_subject(&self, subject: &RdfTerm) -> Vec<Triple> {
        self.triples_matching(Some(subject), None, None)
    }

    /// Returns all triples with the given predicate.
    pub fn triples_for_predicate(&self, predicate: &str) -> Vec<Triple> {
        self.triples_matching(None, Some(predicate), None)
    }

    /// Returns all triples with the given object.
    pub fn triples_for_object(&self, object: &RdfTerm) -> Vec<Triple> {
        self.triples_matching(None, None, Some(object))
    }

    /// Returns all objects for triples matching the given subject and predicate.
    pub fn objects_for_subject_predicate(
        &self,
        subject: &RdfTerm,
        predicate: &str,
    ) -> Vec<RdfTerm> {
        self.triples_matching(Some(subject), Some(predicate), None)
            .into_iter()
            .map(|t| t.object)
            .collect()
    }

    /// Returns all subjects for triples matching the given predicate and object.
    pub fn subjects_for_predicate_object(&self, predicate: &str, object: &RdfTerm) -> Vec<RdfTerm> {
        self.triples_matching(None, Some(predicate), Some(object))
            .into_iter()
            .map(|t| t.subject)
            .collect()
    }

    /// Returns all predicates for triples matching the given subject and object.
    pub fn predicates_for_subject_object(
        &self,
        subject: &RdfTerm,
        object: &RdfTerm,
    ) -> Vec<String> {
        self.triples_matching(Some(subject), None, Some(object))
            .into_iter()
            .map(|t| t.predicate)
            .collect()
    }

    fn get_or_create_vertex(
        &mut self,
        term: RdfTerm,
    ) -> Result<(Option<AddVertex>, Uuid), CrdfError> {
        if let Some(ids) = self.term_to_vertex.get(&term) {
            // Return any existing UUID for this term
            Ok((None, *ids.iter().next().unwrap()))
        } else {
            let vertex = AddVertex {
                id: Uuid::now_v7(),
                term: term.clone(),
            };
            let id = vertex.id;
            self.graph.prepare(vertex.clone().into())?;
            self.term_to_vertex.entry(term).or_default().insert(id);
            Ok((Some(vertex), id))
        }
    }

    /// Converts this graph into an `oxrdf::Graph`.
    pub fn to_oxrdf(&self) -> Result<oxrdf::Graph, CrdfError> {
        let mut graph = oxrdf::Graph::default();
        for triple in self.triples() {
            let oxrdf_triple = triple.to_oxrdf()?;
            graph.insert(oxrdf_triple.as_ref());
        }
        Ok(graph)
    }

    /// Writes this graph as an RDF file.
    pub fn write_rdf_file<P: AsRef<Path>>(
        &self,
        path: P,
        format: RdfFileFormat,
    ) -> Result<(), CrdfError> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        match format {
            RdfFileFormat::NTriples => {
                let graph = self.to_oxrdf()?;
                for triple in &graph {
                    writeln!(writer, "{triple}")?;
                }
            }
            RdfFileFormat::FlatBuffers => {
                let buf = crate::flatbuffers::encode(self);
                writer.write_all(&buf)?;
            }
        }

        writer.flush()?;
        Ok(())
    }

    /// Reads an RDF graph from a FlatBuffers file.
    pub fn read_flatbuffers_file<P: AsRef<Path>>(path: P) -> Result<Self, CrdfError> {
        let buf = std::fs::read(path)?;
        Ok(crate::flatbuffers::decode(&buf)?)
    }
}

impl TryFrom<&RdfGraph> for oxrdf::Graph {
    type Error = CrdfError;

    fn try_from(value: &RdfGraph) -> Result<Self, Self::Error> {
        value.to_oxrdf()
    }
}

impl RdfGraph {
    /// Creates an `RdfGraph` by importing all triples from an `oxrdf::Graph`.
    pub fn from_oxrdf(graph: &oxrdf::Graph) -> Result<Self, CrdfError> {
        let mut rdf_graph = Self::new();
        for triple_ref in graph {
            let subject: RdfTerm = triple_ref.subject.into();
            let predicate = triple_ref.predicate.as_str();
            let object: RdfTerm = triple_ref.object.into();
            rdf_graph.add_triple(subject, predicate, object)?;
        }
        Ok(rdf_graph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::RdfTerm;

    const FOAF_NAME: &str = "http://xmlns.com/foaf/0.1/name";
    const FOAF_KNOWS: &str = "http://xmlns.com/foaf/0.1/knows";
    const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    const FOAF_PERSON: &str = "http://xmlns.com/foaf/0.1/Person";

    fn alice() -> RdfTerm {
        RdfTerm::iri("http://example.org/alice")
    }

    fn bob() -> RdfTerm {
        RdfTerm::iri("http://example.org/bob")
    }

    #[test]
    fn add_and_query_triple() {
        let mut g = RdfGraph::new();
        g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
            .unwrap();

        assert_eq!(g.len(), 1);
        assert!(g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
    }

    #[test]
    fn literal_subject_rejected() {
        let mut g = RdfGraph::new();
        let result = g.add_triple(RdfTerm::literal("bad"), FOAF_NAME, alice());
        assert!(result.is_err());
    }

    #[test]
    fn remove_triple() {
        let mut g = RdfGraph::new();
        g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
            .unwrap();

        g.remove_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice"))
            .unwrap();

        assert_eq!(g.len(), 0);
        assert!(!g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
    }

    #[test]
    fn triples_matching_by_subject() {
        let mut g = RdfGraph::new();
        g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
            .unwrap();
        g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
        g.add_triple(bob(), FOAF_NAME, RdfTerm::literal("Bob"))
            .unwrap();

        let alice_triples = g.triples_matching(Some(&alice()), None, None);
        assert_eq!(alice_triples.len(), 2);
    }

    #[test]
    fn triples_matching_by_predicate() {
        let mut g = RdfGraph::new();
        g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
            .unwrap();
        g.add_triple(bob(), FOAF_NAME, RdfTerm::literal("Bob"))
            .unwrap();
        g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();

        let name_triples = g.triples_matching(None, Some(FOAF_NAME), None);
        assert_eq!(name_triples.len(), 2);
    }

    #[test]
    fn shared_vertex_deduplication() {
        let mut g = RdfGraph::new();
        g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
            .unwrap();
        g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();

        // "alice" vertex should be created only once
        // 3 unique terms = 3 vertex sets
        assert_eq!(g.term_to_vertex.len(), 3);
        // Each term has exactly one UUID
        for ids in g.term_to_vertex.values() {
            assert_eq!(ids.len(), 1);
        }
    }

    #[test]
    fn two_replica_sync() {
        let mut replica_a = RdfGraph::new();
        let mut replica_b = RdfGraph::new();

        // Replica A adds a triple
        let op1 = replica_a
            .add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
            .unwrap();

        // Broadcast to replica B
        replica_b.apply_downstream(op1).unwrap();

        // Both should have the same triple
        assert_eq!(replica_a.len(), 1);
        assert_eq!(replica_b.len(), 1);
        assert!(replica_b.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
    }

    #[test]
    fn two_replica_concurrent_add() {
        let mut replica_a = RdfGraph::new();
        let mut replica_b = RdfGraph::new();

        // Both replicas independently add triples about alice
        let op_a = replica_a
            .add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
            .unwrap();
        let op_b = replica_b
            .add_triple(alice(), RDF_TYPE, RdfTerm::iri(FOAF_PERSON))
            .unwrap();

        // Exchange operations
        replica_b.apply_downstream(op_a).unwrap();
        replica_a.apply_downstream(op_b).unwrap();

        // Both should have 2 triples
        assert_eq!(replica_a.len(), 2);
        assert_eq!(replica_b.len(), 2);
    }

    #[test]
    fn remove_then_sync() {
        let mut replica_a = RdfGraph::new();
        let mut replica_b = RdfGraph::new();

        let op1 = replica_a
            .add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
            .unwrap();
        replica_b.apply_downstream(op1).unwrap();

        let op2 = replica_a
            .remove_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice"))
            .unwrap();
        replica_b.apply_downstream(op2).unwrap();

        assert_eq!(replica_a.len(), 0);
        assert_eq!(replica_b.len(), 0);
    }

    #[test]
    fn blank_node_as_subject() {
        let mut g = RdfGraph::new();
        let bnode = RdfTerm::blank_node("b0");
        g.add_triple(bnode.clone(), FOAF_NAME, RdfTerm::literal("Anonymous"))
            .unwrap();

        assert!(g.contains_triple(&bnode, FOAF_NAME, &RdfTerm::literal("Anonymous")));
    }

    #[test]
    fn triple_display_format() {
        let t = Triple::new(alice(), FOAF_NAME, RdfTerm::literal("Alice"));
        let s = t.to_string();
        assert!(s.contains("<http://example.org/alice>"));
        assert!(s.contains("<http://xmlns.com/foaf/0.1/name>"));
        assert!(s.contains("\"Alice\""));
    }

    #[test]
    fn empty_graph() {
        let g = RdfGraph::new();
        assert!(g.is_empty());
        assert_eq!(g.len(), 0);
        assert!(g.triples().is_empty());
    }
}
