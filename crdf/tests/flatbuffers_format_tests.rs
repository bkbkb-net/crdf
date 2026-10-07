use std::fs;

use crdf::{Literal, RdfFileFormat, RdfGraph, RdfTerm, Uuid};

const EX: &str = "http://example.org/";
const FOAF_NAME: &str = "http://xmlns.com/foaf/0.1/name";
const FOAF_KNOWS: &str = "http://xmlns.com/foaf/0.1/knows";
const FOAF_MBOX: &str = "http://xmlns.com/foaf/0.1/mbox";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const FOAF_PERSON: &str = "http://xmlns.com/foaf/0.1/Person";
const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";

fn tmp_path(ext: &str) -> std::path::PathBuf {
    let name = format!("crdf-test-{}.{ext}", Uuid::now_v7());
    std::env::temp_dir().join(name)
}

fn alice() -> RdfTerm {
    RdfTerm::iri(format!("{EX}alice"))
}
fn bob() -> RdfTerm {
    RdfTerm::iri(format!("{EX}bob"))
}
fn carol() -> RdfTerm {
    RdfTerm::iri(format!("{EX}carol"))
}

// ============================================================
// File I/O roundtrip tests
// ============================================================

#[test]
fn write_and_read_flatbuffers_file() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();

    let path = tmp_path("crdf");
    g.write_rdf_file(&path, RdfFileFormat::FlatBuffers).unwrap();

    let g2 = RdfGraph::read_flatbuffers_file(&path).unwrap();

    assert_eq!(g2.len(), 2);
    assert!(g2.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
    assert!(g2.contains_triple(&alice(), FOAF_KNOWS, &bob()));

    fs::remove_file(path).unwrap();
}

#[test]
fn flatbuffers_file_preserves_crdt_state() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
    g.remove_triple(&alice(), FOAF_KNOWS, &bob()).unwrap();

    // 1 active triple, but CRDT history has 2 edges added + 1 removed
    assert_eq!(g.len(), 1);
    assert_eq!(g.all_edges_added().len(), 2);
    assert_eq!(g.all_edges_removed().len(), 1);

    let path = tmp_path("crdf");
    g.write_rdf_file(&path, RdfFileFormat::FlatBuffers).unwrap();

    let g2 = RdfGraph::read_flatbuffers_file(&path).unwrap();

    assert_eq!(g2.len(), 1);
    assert_eq!(g2.all_vertices_added().len(), g.all_vertices_added().len());
    assert_eq!(
        g2.all_vertices_removed().len(),
        g.all_vertices_removed().len()
    );
    assert_eq!(g2.all_edges_added().len(), 2);
    assert_eq!(g2.all_edges_removed().len(), 1);

    // The surviving triple is still queryable
    assert!(g2.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
    assert!(!g2.contains_triple(&alice(), FOAF_KNOWS, &bob()));

    fs::remove_file(path).unwrap();
}

#[test]
fn flatbuffers_file_preserves_vertex_uuids() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();

    let orig_va: Vec<_> = g
        .all_vertices_added()
        .iter()
        .map(|v| (v.id, v.term.clone()))
        .collect();

    let path = tmp_path("crdf");
    g.write_rdf_file(&path, RdfFileFormat::FlatBuffers).unwrap();
    let g2 = RdfGraph::read_flatbuffers_file(&path).unwrap();

    let loaded_va: Vec<_> = g2
        .all_vertices_added()
        .iter()
        .map(|v| (v.id, v.term.clone()))
        .collect();

    assert_eq!(orig_va, loaded_va);

    fs::remove_file(path).unwrap();
}

#[test]
fn flatbuffers_file_preserves_edge_uuids() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();

    let orig_ea: Vec<_> = g
        .all_edges_added()
        .iter()
        .map(|e| (e.id, e.source, e.target, e.predicate.clone()))
        .collect();

    let path = tmp_path("crdf");
    g.write_rdf_file(&path, RdfFileFormat::FlatBuffers).unwrap();
    let g2 = RdfGraph::read_flatbuffers_file(&path).unwrap();

    let loaded_ea: Vec<_> = g2
        .all_edges_added()
        .iter()
        .map(|e| (e.id, e.source, e.target, e.predicate.clone()))
        .collect();

    assert_eq!(orig_ea, loaded_ea);

    fs::remove_file(path).unwrap();
}

// ============================================================
// In-memory encode/decode tests
// ============================================================

#[test]
fn encode_decode_empty_graph() {
    let g = RdfGraph::new();
    let buf = crdf::flatbuffers::encode(&g);
    let g2 = crdf::flatbuffers::decode(&buf).unwrap();
    assert!(g2.is_empty());
    assert_eq!(g2.all_vertices_added().len(), 0);
}

#[test]
fn encode_decode_all_term_types() {
    let mut g = RdfGraph::new();

    // IRI subject → literal object
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    // IRI subject → IRI object
    g.add_triple(alice(), RDF_TYPE, RdfTerm::iri(FOAF_PERSON))
        .unwrap();
    // Blank node subject → literal object
    g.add_triple(
        RdfTerm::blank_node("b0"),
        FOAF_NAME,
        RdfTerm::literal("Anonymous"),
    )
    .unwrap();
    // Typed literal object
    let int_lit = Literal::new("42").with_datatype(XSD_INTEGER).unwrap();
    g.add_triple(alice(), format!("{EX}age"), RdfTerm::Literal(int_lit))
        .unwrap();
    // Language-tagged literal object
    let ja_lit = Literal::new("アリス").with_language("ja").unwrap();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::Literal(ja_lit))
        .unwrap();

    let buf = crdf::flatbuffers::encode(&g);
    let g2 = crdf::flatbuffers::decode(&buf).unwrap();

    assert_eq!(g2.len(), 5);

    assert!(g2.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
    assert!(g2.contains_triple(&alice(), RDF_TYPE, &RdfTerm::iri(FOAF_PERSON)));
    assert!(g2.contains_triple(
        &RdfTerm::blank_node("b0"),
        FOAF_NAME,
        &RdfTerm::literal("Anonymous"),
    ));

    // Verify typed literal
    let age_triples = g2.triples_matching(Some(&alice()), Some(&format!("{EX}age")), None);
    assert_eq!(age_triples.len(), 1);
    match &age_triples[0].object {
        RdfTerm::Literal(l) => {
            assert_eq!(l.value(), "42");
            assert_eq!(l.datatype(), XSD_INTEGER);
            assert!(l.language().is_none());
        }
        _ => panic!("Expected typed literal"),
    }

    // Verify language-tagged literal
    let ja_triples: Vec<_> = g2
        .triples_for_subject(&alice())
        .into_iter()
        .filter(|t| matches!(&t.object, RdfTerm::Literal(l) if l.language() == Some("ja")))
        .collect();
    assert_eq!(ja_triples.len(), 1);
    match &ja_triples[0].object {
        RdfTerm::Literal(l) => {
            assert_eq!(l.value(), "アリス");
            assert_eq!(l.language(), Some("ja"));
        }
        _ => panic!("Expected language-tagged literal"),
    }
}

#[test]
fn encode_decode_complex_crdt_history() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
    g.add_triple(alice(), FOAF_KNOWS, carol()).unwrap();
    g.add_triple(bob(), FOAF_NAME, RdfTerm::literal("Bob"))
        .unwrap();

    // Remove one edge
    g.remove_triple(&alice(), FOAF_KNOWS, &carol()).unwrap();

    // Re-add the same triple (creates new CRDT edge)
    g.add_triple(alice(), FOAF_KNOWS, carol()).unwrap();

    assert_eq!(g.len(), 3);
    assert_eq!(g.all_edges_added().len(), 4); // 3 original + 1 re-add
    assert_eq!(g.all_edges_removed().len(), 1);

    let buf = crdf::flatbuffers::encode(&g);
    let g2 = crdf::flatbuffers::decode(&buf).unwrap();

    assert_eq!(g2.len(), 3);
    assert_eq!(g2.all_edges_added().len(), 4);
    assert_eq!(g2.all_edges_removed().len(), 1);
    assert!(g2.contains_triple(&alice(), FOAF_KNOWS, &bob()));
    assert!(g2.contains_triple(&alice(), FOAF_KNOWS, &carol()));
    assert!(g2.contains_triple(&bob(), FOAF_NAME, &RdfTerm::literal("Bob")));
}

#[test]
fn encode_decode_removed_vertices() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
    g.remove_triple(&alice(), FOAF_KNOWS, &bob()).unwrap();

    // Graph is empty but CRDT state retains history
    assert_eq!(g.len(), 0);
    let va_count = g.all_vertices_added().len();
    let ea_count = g.all_edges_added().len();
    let er_count = g.all_edges_removed().len();

    let buf = crdf::flatbuffers::encode(&g);
    let g2 = crdf::flatbuffers::decode(&buf).unwrap();

    assert_eq!(g2.len(), 0);
    assert_eq!(g2.all_vertices_added().len(), va_count);
    assert_eq!(g2.all_edges_added().len(), ea_count);
    assert_eq!(g2.all_edges_removed().len(), er_count);
}

// ============================================================
// File format: FlatBuffers binary structure checks
// ============================================================

#[test]
fn flatbuffers_binary_has_crd3_identifier() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();

    let buf = crdf::flatbuffers::encode(&g);

    // FlatBuffers file identifier is at bytes 4..8
    assert!(buf.len() >= 8);
    assert_eq!(&buf[4..8], b"CRD3");
}

#[test]
fn flatbuffers_decode_rejects_garbage() {
    let garbage = b"not a valid flatbuffer at all";
    let result = crdf::flatbuffers::decode(garbage);
    assert!(result.is_err());
}

#[test]
fn flatbuffers_decode_rejects_empty_buffer() {
    let result = crdf::flatbuffers::decode(&[]);
    assert!(result.is_err());
}

// ============================================================
// File I/O edge cases
// ============================================================

#[test]
fn write_flatbuffers_read_back_many_triples() {
    let mut g = RdfGraph::new();
    for i in 0..50 {
        g.add_triple(
            RdfTerm::iri(format!("{EX}node{i}")),
            format!("{EX}edge{i}"),
            RdfTerm::literal(format!("value-{i}")),
        )
        .unwrap();
    }

    let path = tmp_path("crdf");
    g.write_rdf_file(&path, RdfFileFormat::FlatBuffers).unwrap();
    let g2 = RdfGraph::read_flatbuffers_file(&path).unwrap();

    assert_eq!(g2.len(), 50);
    for i in 0..50 {
        assert!(g2.contains_triple(
            &RdfTerm::iri(format!("{EX}node{i}")),
            &format!("{EX}edge{i}"),
            &RdfTerm::literal(format!("value-{i}")),
        ));
    }

    fs::remove_file(path).unwrap();
}

#[test]
fn write_flatbuffers_produces_nonempty_file() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();

    let path = tmp_path("crdf");
    g.write_rdf_file(&path, RdfFileFormat::FlatBuffers).unwrap();

    let metadata = fs::metadata(&path).unwrap();
    assert!(metadata.len() > 0);

    fs::remove_file(path).unwrap();
}

#[test]
fn read_nonexistent_flatbuffers_file_returns_error() {
    let path = tmp_path("crdf");
    let result = RdfGraph::read_flatbuffers_file(&path);
    assert!(result.is_err());
}

// ============================================================
// Interop: FlatBuffers ↔ N-Triples same graph
// ============================================================

#[test]
fn flatbuffers_and_ntriples_produce_same_triples() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
    g.add_triple(bob(), FOAF_NAME, RdfTerm::literal("Bob"))
        .unwrap();
    g.add_triple(bob(), FOAF_MBOX, RdfTerm::iri("mailto:bob@example.org"))
        .unwrap();

    // Save as FlatBuffers, reload
    let fb_path = tmp_path("crdf");
    g.write_rdf_file(&fb_path, RdfFileFormat::FlatBuffers)
        .unwrap();
    let fb_graph = RdfGraph::read_flatbuffers_file(&fb_path).unwrap();

    // Save as N-Triples, reload via oxrdf
    let nt_path = tmp_path("nt");
    g.write_rdf_file(&nt_path, RdfFileFormat::NTriples).unwrap();
    let nt_content = fs::read_to_string(&nt_path).unwrap();

    // Both have same number of triples
    assert_eq!(fb_graph.len(), g.len());

    // FlatBuffers graph contains every original triple
    for t in g.triples() {
        assert!(
            fb_graph.contains_triple(&t.subject, &t.predicate, &t.object),
            "FlatBuffers graph missing: {t}"
        );
    }

    // N-Triples output contains every triple's terms
    assert!(nt_content.contains("http://example.org/alice"));
    assert!(nt_content.contains("http://example.org/bob"));
    assert!(nt_content.contains("\"Alice\""));
    assert!(nt_content.contains("\"Bob\""));

    // FlatBuffers preserves CRDT history, N-Triples does not
    assert!(!fb_graph.all_vertices_added().is_empty());
    assert!(!fb_graph.all_edges_added().is_empty());

    fs::remove_file(fb_path).unwrap();
    fs::remove_file(nt_path).unwrap();
}

// ============================================================
// Replica sync → save → load roundtrip
// ============================================================

#[test]
fn two_replica_state_survives_flatbuffers_roundtrip() {
    let mut replica_a = RdfGraph::new();
    let mut replica_b = RdfGraph::new();

    let op1 = replica_a
        .add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    replica_b.apply_downstream(op1).unwrap();

    let op2 = replica_b
        .add_triple(bob(), FOAF_NAME, RdfTerm::literal("Bob"))
        .unwrap();
    replica_a.apply_downstream(op2).unwrap();

    // Both replicas converged — save replica_a
    assert_eq!(replica_a.len(), 2);

    let path = tmp_path("crdf");
    replica_a
        .write_rdf_file(&path, RdfFileFormat::FlatBuffers)
        .unwrap();
    let loaded = RdfGraph::read_flatbuffers_file(&path).unwrap();

    assert_eq!(loaded.len(), 2);
    assert!(loaded.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
    assert!(loaded.contains_triple(&bob(), FOAF_NAME, &RdfTerm::literal("Bob")));

    // CRDT state preserved
    assert_eq!(
        loaded.all_vertices_added().len(),
        replica_a.all_vertices_added().len()
    );
    assert_eq!(
        loaded.all_edges_added().len(),
        replica_a.all_edges_added().len()
    );

    fs::remove_file(path).unwrap();
}

#[test]
fn loaded_graph_can_continue_operations() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();

    let path = tmp_path("crdf");
    g.write_rdf_file(&path, RdfFileFormat::FlatBuffers).unwrap();
    let mut g2 = RdfGraph::read_flatbuffers_file(&path).unwrap();

    // Add more triples to loaded graph
    g2.add_triple(bob(), FOAF_KNOWS, carol()).unwrap();
    g2.add_triple(carol(), FOAF_NAME, RdfTerm::literal("Carol"))
        .unwrap();

    assert_eq!(g2.len(), 3);
    assert!(g2.contains_triple(&alice(), FOAF_KNOWS, &bob()));
    assert!(g2.contains_triple(&bob(), FOAF_KNOWS, &carol()));
    assert!(g2.contains_triple(&carol(), FOAF_NAME, &RdfTerm::literal("Carol")));

    // Can also remove from loaded graph
    g2.remove_triple(&alice(), FOAF_KNOWS, &bob()).unwrap();
    assert_eq!(g2.len(), 2);

    fs::remove_file(path).unwrap();
}

#[test]
fn save_load_save_load_preserves_state() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();

    // First save/load cycle
    let path1 = tmp_path("crdf");
    g.write_rdf_file(&path1, RdfFileFormat::FlatBuffers)
        .unwrap();
    let mut g2 = RdfGraph::read_flatbuffers_file(&path1).unwrap();

    // Modify and save again
    g2.add_triple(carol(), FOAF_NAME, RdfTerm::literal("Carol"))
        .unwrap();

    let path2 = tmp_path("crdf");
    g2.write_rdf_file(&path2, RdfFileFormat::FlatBuffers)
        .unwrap();
    let g3 = RdfGraph::read_flatbuffers_file(&path2).unwrap();

    assert_eq!(g3.len(), 2);
    assert!(g3.contains_triple(&alice(), FOAF_KNOWS, &bob()));
    assert!(g3.contains_triple(&carol(), FOAF_NAME, &RdfTerm::literal("Carol")));

    // Full CRDT history from both sessions
    assert_eq!(g3.all_vertices_added().len(), g2.all_vertices_added().len());
    assert_eq!(g3.all_edges_added().len(), g2.all_edges_added().len());

    fs::remove_file(path1).unwrap();
    fs::remove_file(path2).unwrap();
}

// ============================================================
// Unicode & special characters
// ============================================================

#[test]
fn encode_decode_unicode_content() {
    let mut g = RdfGraph::new();
    let lit_jp = Literal::new("東京タワー").with_language("ja").unwrap();
    let lit_emoji = RdfTerm::literal("Hello 🌍🚀");
    let iri_unicode = RdfTerm::iri("http://example.org/日本語");

    g.add_triple(iri_unicode.clone(), FOAF_NAME, RdfTerm::Literal(lit_jp))
        .unwrap();
    g.add_triple(iri_unicode.clone(), format!("{EX}desc"), lit_emoji.clone())
        .unwrap();

    let buf = crdf::flatbuffers::encode(&g);
    let g2 = crdf::flatbuffers::decode(&buf).unwrap();

    assert_eq!(g2.len(), 2);

    let names = g2.triples_for_subject(&iri_unicode);
    assert_eq!(names.len(), 2);

    let ja_triple = names
        .iter()
        .find(|t| matches!(&t.object, RdfTerm::Literal(l) if l.language() == Some("ja")))
        .expect("Should find Japanese literal");
    match &ja_triple.object {
        RdfTerm::Literal(l) => assert_eq!(l.value(), "東京タワー"),
        _ => panic!("Expected literal"),
    }

    assert!(g2.contains_triple(&iri_unicode, &format!("{EX}desc"), &lit_emoji));
}

#[test]
fn encode_decode_special_characters_in_literals() {
    let mut g = RdfGraph::new();
    // Newlines, tabs, quotes, backslashes in literal values
    let tricky = RdfTerm::literal("line1\nline2\ttab\\slash\"quote");
    g.add_triple(alice(), format!("{EX}note"), tricky.clone())
        .unwrap();

    let buf = crdf::flatbuffers::encode(&g);
    let g2 = crdf::flatbuffers::decode(&buf).unwrap();

    assert_eq!(g2.len(), 1);
    assert!(g2.contains_triple(&alice(), &format!("{EX}note"), &tricky));
}

// ============================================================
// Fixture file tests (tests/fixtures/people.crdf)
// ============================================================

const PEOPLE_CRDF: &str = "tests/fixtures/people.crdf";
const FOAF_AGE: &str = "http://xmlns.com/foaf/0.1/age";

fn load_people() -> RdfGraph {
    RdfGraph::read_flatbuffers_file(PEOPLE_CRDF).expect("Failed to load people.crdf fixture")
}

#[test]
fn fixture_loads_successfully() {
    let g = load_people();
    assert_eq!(g.len(), 4);
}

#[test]
fn fixture_contains_expected_triples() {
    let g = load_people();

    assert!(g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
    assert!(g.contains_triple(&alice(), FOAF_KNOWS, &bob()));
    assert!(g.contains_triple(&bob(), FOAF_NAME, &RdfTerm::literal("Bob")));
    assert!(g.contains_triple(&bob(), FOAF_MBOX, &RdfTerm::iri("mailto:bob@example.org")));
}

#[test]
fn fixture_does_not_contain_removed_triple() {
    let g = load_people();

    // The age triple was added then removed during generation
    let age_lit = RdfTerm::Literal(Literal::new("30").with_datatype(XSD_INTEGER).unwrap());
    assert!(!g.contains_triple(&alice(), FOAF_AGE, &age_lit));
}

#[test]
fn fixture_preserves_crdt_history() {
    let g = load_people();

    // 6 vertices added: alice, "Alice", bob, "Bob", mailto:bob@example.org, "30"^^xsd:integer
    assert_eq!(g.all_vertices_added().len(), 6);
    // 0 vertices removed
    assert_eq!(g.all_vertices_removed().len(), 0);
    // 5 edges added: 4 active triples + the removed age triple
    assert_eq!(g.all_edges_added().len(), 5);
    // 1 edge removed: the age triple
    assert_eq!(g.all_edges_removed().len(), 1);
}

#[test]
fn fixture_query_by_subject() {
    let g = load_people();

    let alice_triples = g.triples_for_subject(&alice());
    assert_eq!(alice_triples.len(), 2); // name + knows

    let bob_triples = g.triples_for_subject(&bob());
    assert_eq!(bob_triples.len(), 2); // name + mbox
}

#[test]
fn fixture_query_by_predicate() {
    let g = load_people();

    let name_triples = g.triples_for_predicate(FOAF_NAME);
    assert_eq!(name_triples.len(), 2); // Alice + Bob

    let knows_triples = g.triples_for_predicate(FOAF_KNOWS);
    assert_eq!(knows_triples.len(), 1); // alice knows bob
}

#[test]
fn fixture_subjects_and_predicates() {
    let g = load_people();

    let subjects = g.subjects();
    assert_eq!(subjects.len(), 2); // alice, bob

    let predicates = g.predicates();
    assert_eq!(predicates.len(), 3); // name, knows, mbox
}

#[test]
fn fixture_matches_ntriples_content() {
    let crdf_graph = load_people();

    // Build same graph from people.nt content
    let nt_content = fs::read_to_string("tests/fixtures/people.nt").unwrap();
    let mut nt_graph = RdfGraph::new();
    for line in nt_content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Minimal N-Triples parser for this known content
        let line = line.strip_suffix('.').unwrap().trim();
        let parts: Vec<&str> = line.splitn(3, ' ').collect();
        let subject = RdfTerm::iri(parts[0].trim_matches(|c| c == '<' || c == '>'));
        let predicate = parts[1].trim_matches(|c| c == '<' || c == '>');
        let obj_str = parts[2].trim();
        let object = if obj_str.starts_with('<') {
            RdfTerm::iri(obj_str.trim_matches(|c| c == '<' || c == '>'))
        } else {
            // "value"
            let val = obj_str.trim_matches('"');
            RdfTerm::literal(val)
        };
        nt_graph.add_triple(subject, predicate, object).unwrap();
    }

    // Same active triples
    assert_eq!(crdf_graph.len(), nt_graph.len());
    for t in nt_graph.triples() {
        assert!(
            crdf_graph.contains_triple(&t.subject, &t.predicate, &t.object),
            "CRDF graph missing triple from NT: {t}"
        );
    }

    // CRDF has richer CRDT history (extra add+remove of age)
    assert!(crdf_graph.all_edges_added().len() > nt_graph.all_edges_added().len());
    assert!(crdf_graph.all_edges_removed().len() > nt_graph.all_edges_removed().len());
}

#[test]
fn fixture_can_be_modified_after_loading() {
    let mut g = load_people();
    assert_eq!(g.len(), 4);

    g.add_triple(carol(), FOAF_NAME, RdfTerm::literal("Carol"))
        .unwrap();
    assert_eq!(g.len(), 5);
    assert!(g.contains_triple(&carol(), FOAF_NAME, &RdfTerm::literal("Carol")));

    g.remove_triple(&alice(), FOAF_KNOWS, &bob()).unwrap();
    assert_eq!(g.len(), 4);
    assert!(!g.contains_triple(&alice(), FOAF_KNOWS, &bob()));
}

#[test]
fn fixture_resave_roundtrip() {
    let g = load_people();

    let path = tmp_path("crdf");
    g.write_rdf_file(&path, RdfFileFormat::FlatBuffers).unwrap();

    let g2 = RdfGraph::read_flatbuffers_file(&path).unwrap();

    assert_eq!(g2.len(), g.len());
    assert_eq!(g2.all_vertices_added().len(), g.all_vertices_added().len());
    assert_eq!(g2.all_edges_added().len(), g.all_edges_added().len());
    assert_eq!(g2.all_edges_removed().len(), g.all_edges_removed().len());

    for t in g.triples() {
        assert!(g2.contains_triple(&t.subject, &t.predicate, &t.object));
    }

    fs::remove_file(path).unwrap();
}
