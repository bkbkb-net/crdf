use crdt_graph::UpdateOperation;
use crdt_graph::flatbuffers::DecodeError;
use crdt_graph::flatbuffers::string as fb_string;
use crdt_graph::types::string as str_types;
use crdt_graph::types::{RemoveEdge, RemoveVertex};

use crate::term::{Literal, RDF_LANG_STRING, RdfTerm, XSD_STRING};
use crate::types::{AddEdge, AddVertex, GraphOperation};

// ---------------------------------------------------------------------------
// RdfTerm ↔ String encoding
// ---------------------------------------------------------------------------

/// Serialises an [`RdfTerm`] into a compact string representation.
///
/// Format:
/// - `I<iri>`            – IRI
/// - `B<id>`             – Blank node
/// - `L<value>`          – xsd:string literal (no language)
/// - `T<datatype>\n<value>` – typed literal
/// - `G<lang>\n<value>`  – language-tagged literal
fn encode_term(term: &RdfTerm) -> String {
    match term {
        RdfTerm::Iri(iri) => format!("I{iri}"),
        RdfTerm::BlankNode(id) => format!("B{id}"),
        RdfTerm::Literal(lit) => {
            if let Some(lang) = &lit.language {
                format!("G{lang}\n{}", lit.value)
            } else if lit.datatype == XSD_STRING {
                format!("L{}", lit.value)
            } else {
                format!("T{}\n{}", lit.datatype, lit.value)
            }
        }
    }
}

/// Deserialises an [`RdfTerm`] from the compact string representation.
fn decode_term(s: &str) -> Result<RdfTerm, DecodeError> {
    if s.is_empty() {
        return Err(DecodeError::UnknownOperationType);
    }
    let (tag, rest) = s.split_at(1);
    match tag {
        "I" => Ok(RdfTerm::Iri(rest.to_string())),
        "B" => Ok(RdfTerm::BlankNode(rest.to_string())),
        "L" => Ok(RdfTerm::Literal(Literal::new(rest))),
        "T" => {
            let (datatype, value) = rest
                .split_once('\n')
                .ok_or(DecodeError::UnknownOperationType)?;
            Ok(RdfTerm::Literal(Literal {
                value: value.to_string(),
                datatype: datatype.to_string(),
                language: None,
            }))
        }
        "G" => {
            let (lang, value) = rest
                .split_once('\n')
                .ok_or(DecodeError::UnknownOperationType)?;
            Ok(RdfTerm::Literal(Literal {
                value: value.to_string(),
                datatype: RDF_LANG_STRING.to_string(),
                language: Some(lang.to_string()),
            }))
        }
        _ => Err(DecodeError::UnknownOperationType),
    }
}

// ---------------------------------------------------------------------------
// crdf types ↔ crdt-graph string types conversion
// ---------------------------------------------------------------------------

fn crdf_to_str_ops(
    va: &[AddVertex],
    vr: &[RemoveVertex],
    ea: &[AddEdge],
    er: &[RemoveEdge],
) -> Vec<str_types::Operation> {
    let mut ops = Vec::with_capacity(va.len() + vr.len() + ea.len() + er.len());

    for v in va {
        ops.push(UpdateOperation::AddVertex(str_types::AddVertex {
            id: v.id,
            data: Some(encode_term(&v.term)),
        }));
    }
    for v in vr {
        ops.push(UpdateOperation::RemoveVertex(RemoveVertex {
            id: v.id,
            add_vertex_id: v.add_vertex_id,
        }));
    }
    for e in ea {
        ops.push(UpdateOperation::AddEdge(str_types::AddEdge {
            id: e.id,
            source: e.source,
            target: e.target,
            data: Some(e.predicate.clone()),
        }));
    }
    for e in er {
        ops.push(UpdateOperation::RemoveEdge(RemoveEdge {
            id: e.id,
            add_edge_id: e.add_edge_id,
        }));
    }

    ops
}

fn str_op_to_crdf(op: str_types::Operation) -> Result<GraphOperation, DecodeError> {
    match op {
        UpdateOperation::AddVertex(v) => {
            let term = decode_term(v.data.as_deref().ok_or(DecodeError::UnknownOperationType)?)?;
            Ok(UpdateOperation::AddVertex(AddVertex { id: v.id, term }))
        }
        UpdateOperation::RemoveVertex(v) => Ok(UpdateOperation::RemoveVertex(v)),
        UpdateOperation::AddEdge(e) => {
            let predicate = e.data.ok_or(DecodeError::UnknownOperationType)?;
            Ok(UpdateOperation::AddEdge(AddEdge {
                id: e.id,
                source: e.source,
                target: e.target,
                predicate,
            }))
        }
        UpdateOperation::RemoveEdge(e) => Ok(UpdateOperation::RemoveEdge(e)),
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Encodes the CRDT state of an [`RdfGraph`](crate::RdfGraph) into a FlatBuffer byte vector
/// using the `crdt_graph_with_str_data` schema (file identifier `"CRD3"`).
///
/// The entire CRDT operation history (V_A, V_R, E_A, E_R) is preserved,
/// enabling full reconstruction of the graph including undo history.
pub fn encode(graph: &crate::RdfGraph) -> Vec<u8> {
    let ops = crdf_to_str_ops(
        graph.all_vertices_added(),
        graph.all_vertices_removed(),
        graph.all_edges_added(),
        graph.all_edges_removed(),
    );
    fb_string::encode_operation_log(&ops)
}

/// Decodes a FlatBuffer byte vector into an [`RdfGraph`](crate::RdfGraph),
/// replaying the full CRDT operation history.
///
/// # Errors
/// Returns a [`DecodeError`] if the buffer is invalid or contains unknown operation types.
pub fn decode(buf: &[u8]) -> Result<crate::RdfGraph, DecodeError> {
    let str_ops = fb_string::decode_operation_log(buf)?;

    let mut graph = crate::types::Graph::new();
    let mut term_to_vertex =
        std::collections::HashMap::<RdfTerm, std::collections::HashSet<uuid::Uuid>>::new();

    for str_op in str_ops {
        let crdf_op = str_op_to_crdf(str_op)?;
        if let UpdateOperation::AddVertex(v) = &crdf_op {
            term_to_vertex
                .entry(v.term.clone())
                .or_default()
                .insert(v.id);
        }
        graph
            .apply_downstream(crdf_op)
            .map_err(|_| DecodeError::UnknownOperationType)?;
    }

    Ok(crate::RdfGraph::from_raw(graph, term_to_vertex))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RdfGraph;

    #[test]
    fn roundtrip_empty_graph() {
        let g = RdfGraph::new();
        let buf = encode(&g);
        let g2 = decode(&buf).unwrap();
        assert!(g2.is_empty());
    }

    #[test]
    fn roundtrip_single_triple() {
        let mut g = RdfGraph::new();
        g.add_triple(
            RdfTerm::iri("http://example.org/Alice"),
            "http://xmlns.com/foaf/0.1/knows",
            RdfTerm::iri("http://example.org/Bob"),
        )
        .unwrap();

        let buf = encode(&g);
        let g2 = decode(&buf).unwrap();

        assert_eq!(g2.len(), 1);
        assert!(g2.contains_triple(
            &RdfTerm::iri("http://example.org/Alice"),
            "http://xmlns.com/foaf/0.1/knows",
            &RdfTerm::iri("http://example.org/Bob"),
        ));
    }

    #[test]
    fn roundtrip_preserves_crdt_history() {
        let mut g = RdfGraph::new();
        g.add_triple(
            RdfTerm::iri("http://example.org/Alice"),
            "http://xmlns.com/foaf/0.1/knows",
            RdfTerm::iri("http://example.org/Bob"),
        )
        .unwrap();
        g.remove_triple(
            &RdfTerm::iri("http://example.org/Alice"),
            "http://xmlns.com/foaf/0.1/knows",
            &RdfTerm::iri("http://example.org/Bob"),
        )
        .unwrap();

        assert_eq!(g.len(), 0);
        assert_eq!(g.all_vertices_added().len(), 2);
        assert_eq!(g.all_edges_added().len(), 1);
        assert_eq!(g.all_edges_removed().len(), 1);

        let buf = encode(&g);
        let g2 = decode(&buf).unwrap();

        assert_eq!(g2.len(), 0);
        assert_eq!(g2.all_vertices_added().len(), 2);
        assert_eq!(g2.all_edges_added().len(), 1);
        assert_eq!(g2.all_edges_removed().len(), 1);
    }

    #[test]
    fn roundtrip_literal_with_datatype() {
        let mut g = RdfGraph::new();
        let lit = Literal::new("42")
            .with_datatype("http://www.w3.org/2001/XMLSchema#integer")
            .unwrap();
        g.add_triple(
            RdfTerm::iri("http://example.org/x"),
            "http://example.org/value",
            RdfTerm::Literal(lit),
        )
        .unwrap();

        let buf = encode(&g);
        let g2 = decode(&buf).unwrap();

        let triples = g2.triples();
        assert_eq!(triples.len(), 1);
        let obj = &triples[0].object;
        match obj {
            RdfTerm::Literal(l) => {
                assert_eq!(l.value(), "42");
                assert_eq!(l.datatype(), "http://www.w3.org/2001/XMLSchema#integer");
            }
            _ => panic!("Expected literal"),
        }
    }

    #[test]
    fn roundtrip_language_tagged_literal() {
        let mut g = RdfGraph::new();
        let lit = Literal::new("こんにちは").with_language("ja").unwrap();
        g.add_triple(
            RdfTerm::iri("http://example.org/x"),
            "http://example.org/label",
            RdfTerm::Literal(lit),
        )
        .unwrap();

        let buf = encode(&g);
        let g2 = decode(&buf).unwrap();

        let triples = g2.triples();
        assert_eq!(triples.len(), 1);
        match &triples[0].object {
            RdfTerm::Literal(l) => {
                assert_eq!(l.value(), "こんにちは");
                assert_eq!(l.language(), Some("ja"));
            }
            _ => panic!("Expected literal"),
        }
    }

    #[test]
    fn roundtrip_blank_node() {
        let mut g = RdfGraph::new();
        g.add_triple(
            RdfTerm::blank_node("b0"),
            "http://example.org/name",
            RdfTerm::literal("Anonymous"),
        )
        .unwrap();

        let buf = encode(&g);
        let g2 = decode(&buf).unwrap();

        assert_eq!(g2.len(), 1);
        assert!(g2.contains_triple(
            &RdfTerm::blank_node("b0"),
            "http://example.org/name",
            &RdfTerm::literal("Anonymous"),
        ));
    }

    #[test]
    fn roundtrip_multiple_triples() {
        let mut g = RdfGraph::new();
        let alice = RdfTerm::iri("http://example.org/Alice");
        let bob = RdfTerm::iri("http://example.org/Bob");
        let carol = RdfTerm::iri("http://example.org/Carol");

        g.add_triple(
            alice.clone(),
            "http://xmlns.com/foaf/0.1/knows",
            bob.clone(),
        )
        .unwrap();
        g.add_triple(
            alice.clone(),
            "http://xmlns.com/foaf/0.1/knows",
            carol.clone(),
        )
        .unwrap();
        g.add_triple(
            bob.clone(),
            "http://xmlns.com/foaf/0.1/name",
            RdfTerm::literal("Bob"),
        )
        .unwrap();

        let buf = encode(&g);
        let g2 = decode(&buf).unwrap();

        assert_eq!(g2.len(), 3);
        assert!(g2.contains_triple(&alice, "http://xmlns.com/foaf/0.1/knows", &bob));
        assert!(g2.contains_triple(&alice, "http://xmlns.com/foaf/0.1/knows", &carol));
        assert!(g2.contains_triple(
            &bob,
            "http://xmlns.com/foaf/0.1/name",
            &RdfTerm::literal("Bob")
        ));
    }

    #[test]
    fn encode_term_roundtrip() {
        let terms = vec![
            RdfTerm::iri("http://example.org/test"),
            RdfTerm::blank_node("b42"),
            RdfTerm::literal("hello"),
            RdfTerm::Literal(
                Literal::new("42")
                    .with_datatype("http://www.w3.org/2001/XMLSchema#integer")
                    .unwrap(),
            ),
            RdfTerm::Literal(Literal::new("hello").with_language("en").unwrap()),
        ];

        for term in terms {
            let encoded = encode_term(&term);
            let decoded = decode_term(&encoded).unwrap();
            assert_eq!(term, decoded, "Failed roundtrip for: {term:?}");
        }
    }
}
