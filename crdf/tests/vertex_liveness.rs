use crdf::{RdfGraph, RdfTerm};
use crdt_graph::{
    UpdateOperation,
    flatbuffers::string::encode_operation_log,
    types::{
        RemoveVertex,
        string::{AddEdge, AddVertex, Operation},
    },
};
use uuid::Uuid;

fn removed_endpoint(remove_subject: bool) -> RdfGraph {
    let subject = Uuid::now_v7();
    let object = Uuid::now_v7();
    let ops: Vec<Operation> = vec![
        UpdateOperation::AddVertex(AddVertex {
            id: subject,
            data: Some("Iurn:s".into()),
        }),
        UpdateOperation::AddVertex(AddVertex {
            id: object,
            data: Some("Iurn:o".into()),
        }),
        UpdateOperation::AddEdge(AddEdge {
            id: Uuid::now_v7(),
            source: subject,
            target: object,
            data: Some("urn:p".into()),
        }),
        UpdateOperation::RemoveVertex(RemoveVertex {
            id: Uuid::now_v7(),
            add_vertex_id: if remove_subject { subject } else { object },
        }),
    ];
    crdf::flatbuffers::decode(&encode_operation_log(&ops)).unwrap()
}

#[test]
fn removed_endpoints_are_invisible_and_can_be_added_again() {
    for remove_subject in [false, true] {
        let mut graph = removed_endpoint(remove_subject);
        let s = RdfTerm::iri("urn:s");
        let o = RdfTerm::iri("urn:o");
        assert!(graph.triples().is_empty());
        assert!(graph.subjects().is_empty());
        assert!(graph.objects().is_empty());
        assert!(graph.predicates().is_empty());
        assert_eq!(graph.len(), 0);
        assert!(graph.is_empty());
        assert!(!graph.contains_triple(&s, "urn:p", &o));
        assert!(graph.remove_triple(&s, "urn:p", &o).is_err());
        let mut replica = graph.clone();
        let op = graph.add_triple(s.clone(), "urn:p", o.clone()).unwrap();
        replica.apply_downstream(op).unwrap();
        for current in [&graph, &replica] {
            assert_eq!(current.len(), 1);
            assert!(current.contains_triple(&s, "urn:p", &o));
            assert_eq!(current.all_vertices_added().len(), 3);
            assert_eq!(current.all_vertices_removed().len(), 1);
            assert_eq!(current.all_edges_added().len(), 2);
            let decoded = crdf::flatbuffers::decode(&crdf::flatbuffers::encode(current)).unwrap();
            assert_eq!(decoded.triples(), current.triples());
        }
    }
}
