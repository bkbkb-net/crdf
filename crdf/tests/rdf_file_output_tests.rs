use std::fs;

use crdf::{RdfFileFormat, RdfGraph, RdfTerm, Uuid};

const EX_ALICE: &str = "http://example.org/alice";
const EX_BOB: &str = "http://example.org/bob";
const FOAF_NAME: &str = "http://xmlns.com/foaf/0.1/name";
const FOAF_KNOWS: &str = "http://xmlns.com/foaf/0.1/knows";

#[test]
fn write_ntriples_file_from_graph() {
    let mut g = RdfGraph::new();
    g.add_triple(RdfTerm::iri(EX_ALICE), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.add_triple(RdfTerm::iri(EX_ALICE), FOAF_KNOWS, RdfTerm::iri(EX_BOB))
        .unwrap();

    let file_name = format!("crdf-rdf-export-{}.nt", Uuid::now_v7());
    let path = std::env::temp_dir().join(file_name);

    g.write_rdf_file(&path, RdfFileFormat::NTriples).unwrap();

    let text = fs::read_to_string(&path).unwrap();

    assert!(text.contains("<http://example.org/alice> <http://xmlns.com/foaf/0.1/name>"));
    assert!(text.contains("\"Alice\""));
    assert!(text.contains("http://xmlns.com/foaf/0.1/knows"));
    assert!(text.contains("http://example.org/bob"));

    fs::remove_file(path).unwrap();
}
