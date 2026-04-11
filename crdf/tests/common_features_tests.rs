use crdf::{Literal, RdfGraph, RdfTerm};

// ── XSD data type IRIs ────────────────────────────────────────
const XSD_BOOLEAN: &str = "http://www.w3.org/2001/XMLSchema#boolean";
const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
const XSD_INT: &str = "http://www.w3.org/2001/XMLSchema#int";
const XSD_DOUBLE: &str = "http://www.w3.org/2001/XMLSchema#double";
const XSD_FLOAT: &str = "http://www.w3.org/2001/XMLSchema#float";
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

// ── Helpers ───────────────────────────────────────────────────
const FOAF_NAME: &str = "http://xmlns.com/foaf/0.1/name";
const FOAF_KNOWS: &str = "http://xmlns.com/foaf/0.1/knows";
const FOAF_AGE: &str = "http://xmlns.com/foaf/0.1/age";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const FOAF_PERSON: &str = "http://xmlns.com/foaf/0.1/Person";

fn alice() -> RdfTerm {
    RdfTerm::iri("http://example.org/alice")
}
fn bob() -> RdfTerm {
    RdfTerm::iri("http://example.org/bob")
}
fn charlie() -> RdfTerm {
    RdfTerm::iri("http://example.org/charlie")
}

// ════════════════════════════════════════════════════════════════
//  RdfTerm accessor tests
// ════════════════════════════════════════════════════════════════

#[test]
fn as_iri_returns_some_for_iri() {
    let term = RdfTerm::iri("http://example.org/x");
    assert_eq!(term.as_iri(), Some("http://example.org/x"));
}

#[test]
fn as_iri_returns_none_for_blank_node() {
    let term = RdfTerm::blank_node("b0");
    assert_eq!(term.as_iri(), None);
}

#[test]
fn as_iri_returns_none_for_literal() {
    let term = RdfTerm::literal("hello");
    assert_eq!(term.as_iri(), None);
}

#[test]
fn as_blank_node_returns_some() {
    let term = RdfTerm::blank_node("b1");
    assert_eq!(term.as_blank_node(), Some("b1"));
}

#[test]
fn as_blank_node_returns_none_for_iri() {
    let term = RdfTerm::iri("http://example.org/x");
    assert_eq!(term.as_blank_node(), None);
}

#[test]
fn as_literal_returns_some() {
    let term = RdfTerm::literal("hello");
    let lit = term.as_literal().unwrap();
    assert_eq!(lit.value(), "hello");
}

#[test]
fn as_literal_returns_none_for_iri() {
    let term = RdfTerm::iri("http://example.org/x");
    assert!(term.as_literal().is_none());
}

// ════════════════════════════════════════════════════════════════
//  Literal accessor tests
// ════════════════════════════════════════════════════════════════

#[test]
fn literal_value_accessor() {
    let lit = Literal::new("hello");
    assert_eq!(lit.value(), "hello");
}

#[test]
fn literal_datatype_accessor_none() {
    let lit = Literal::new("hello");
    assert_eq!(lit.datatype(), XSD_STRING);
}

#[test]
fn literal_datatype_accessor_some() {
    let lit = Literal::new("42").with_datatype(XSD_INTEGER).unwrap();
    assert_eq!(lit.datatype(), XSD_INTEGER);
}

#[test]
fn literal_language_accessor_none() {
    let lit = Literal::new("hello");
    assert_eq!(lit.language(), None);
}

#[test]
fn literal_language_accessor_some() {
    let lit = Literal::new("hello").with_language("en").unwrap();
    assert_eq!(lit.language(), Some("en"));
}

// ════════════════════════════════════════════════════════════════
//  Literal Display tests
// ════════════════════════════════════════════════════════════════

#[test]
fn literal_display_simple() {
    let lit = Literal::new("hello");
    assert_eq!(lit.to_string(), "\"hello\"");
}

#[test]
fn literal_display_with_language() {
    let lit = Literal::new("hello").with_language("en").unwrap();
    assert_eq!(lit.to_string(), "\"hello\"@en");
}

#[test]
fn literal_display_with_datatype() {
    let lit = Literal::new("42").with_datatype(XSD_INTEGER).unwrap();
    assert_eq!(
        lit.to_string(),
        format!("\"42\"^^<{XSD_INTEGER}>")
    );
}

// ════════════════════════════════════════════════════════════════
//  From<primitive> for Literal
// ════════════════════════════════════════════════════════════════

#[test]
fn literal_from_bool_true() {
    let lit = Literal::from(true);
    assert_eq!(lit.value(), "true");
    assert_eq!(lit.datatype(), XSD_BOOLEAN);
}

#[test]
fn literal_from_bool_false() {
    let lit = Literal::from(false);
    assert_eq!(lit.value(), "false");
    assert_eq!(lit.datatype(), XSD_BOOLEAN);
}

#[test]
fn literal_from_i32() {
    let lit = Literal::from(42i32);
    assert_eq!(lit.value(), "42");
    assert_eq!(lit.datatype(), XSD_INT);
}

#[test]
fn literal_from_i64() {
    let lit = Literal::from(1_000_000i64);
    assert_eq!(lit.value(), "1000000");
    assert_eq!(lit.datatype(), XSD_INTEGER);
}

#[test]
fn literal_from_f64() {
    let lit = Literal::from(3.14f64);
    assert_eq!(lit.value(), "3.14");
    assert_eq!(lit.datatype(), XSD_DOUBLE);
}

#[test]
fn literal_from_f32() {
    let lit = Literal::from(2.5f32);
    assert_eq!(lit.value(), "2.5");
    assert_eq!(lit.datatype(), XSD_FLOAT);
}

#[test]
fn literal_from_str_ref() {
    let lit = Literal::from("hello");
    assert_eq!(lit.value(), "hello");
    assert_eq!(lit.datatype(), XSD_STRING);
}

#[test]
fn literal_from_string() {
    let lit = Literal::from(String::from("world"));
    assert_eq!(lit.value(), "world");
    assert_eq!(lit.datatype(), XSD_STRING);
}

// ════════════════════════════════════════════════════════════════
//  From<Literal> for RdfTerm
// ════════════════════════════════════════════════════════════════

#[test]
fn rdf_term_from_literal() {
    let lit = Literal::from(42i64);
    let term: RdfTerm = lit.clone().into();
    assert!(term.is_literal());
    assert_eq!(term.as_literal().unwrap().value(), "42");
}

// ════════════════════════════════════════════════════════════════
//  Graph convenience query methods
// ════════════════════════════════════════════════════════════════

fn build_sample_graph() -> RdfGraph {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice")).unwrap();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
    g.add_triple(alice(), RDF_TYPE, RdfTerm::iri(FOAF_PERSON)).unwrap();
    g.add_triple(bob(), FOAF_NAME, RdfTerm::literal("Bob")).unwrap();
    g.add_triple(bob(), FOAF_KNOWS, charlie()).unwrap();
    g.add_triple(charlie(), FOAF_NAME, RdfTerm::literal("Charlie")).unwrap();
    g
}

#[test]
fn triples_for_subject_alice() {
    let g = build_sample_graph();
    let triples = g.triples_for_subject(&alice());
    assert_eq!(triples.len(), 3);
    assert!(triples.iter().all(|t| t.subject == alice()));
}

#[test]
fn triples_for_subject_bob() {
    let g = build_sample_graph();
    let triples = g.triples_for_subject(&bob());
    assert_eq!(triples.len(), 2);
}

#[test]
fn triples_for_subject_nonexistent() {
    let g = build_sample_graph();
    let triples = g.triples_for_subject(&RdfTerm::iri("http://example.org/nobody"));
    assert!(triples.is_empty());
}

#[test]
fn triples_for_predicate_knows() {
    let g = build_sample_graph();
    let triples = g.triples_for_predicate(FOAF_KNOWS);
    assert_eq!(triples.len(), 2);
    assert!(triples.iter().all(|t| t.predicate == FOAF_KNOWS));
}

#[test]
fn triples_for_predicate_name() {
    let g = build_sample_graph();
    let triples = g.triples_for_predicate(FOAF_NAME);
    assert_eq!(triples.len(), 3);
}

#[test]
fn triples_for_object_bob() {
    let g = build_sample_graph();
    let triples = g.triples_for_object(&bob());
    assert_eq!(triples.len(), 1);
    assert_eq!(triples[0].subject, alice());
    assert_eq!(triples[0].predicate, FOAF_KNOWS);
}

#[test]
fn triples_for_object_literal() {
    let g = build_sample_graph();
    let triples = g.triples_for_object(&RdfTerm::literal("Alice"));
    assert_eq!(triples.len(), 1);
    assert_eq!(triples[0].subject, alice());
}

#[test]
fn objects_for_subject_predicate_alice_name() {
    let g = build_sample_graph();
    let objects = g.objects_for_subject_predicate(&alice(), FOAF_NAME);
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0], RdfTerm::literal("Alice"));
}

#[test]
fn objects_for_subject_predicate_alice_knows() {
    let g = build_sample_graph();
    let objects = g.objects_for_subject_predicate(&alice(), FOAF_KNOWS);
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0], bob());
}

#[test]
fn objects_for_subject_predicate_no_match() {
    let g = build_sample_graph();
    let objects = g.objects_for_subject_predicate(&charlie(), FOAF_KNOWS);
    assert!(objects.is_empty());
}

#[test]
fn subjects_for_predicate_object_knows_bob() {
    let g = build_sample_graph();
    let subjects = g.subjects_for_predicate_object(FOAF_KNOWS, &bob());
    assert_eq!(subjects.len(), 1);
    assert_eq!(subjects[0], alice());
}

#[test]
fn subjects_for_predicate_object_name_bob() {
    let g = build_sample_graph();
    let subjects = g.subjects_for_predicate_object(FOAF_NAME, &RdfTerm::literal("Bob"));
    assert_eq!(subjects.len(), 1);
    assert_eq!(subjects[0], bob());
}

#[test]
fn subjects_for_predicate_object_type_person() {
    let g = build_sample_graph();
    let subjects = g.subjects_for_predicate_object(RDF_TYPE, &RdfTerm::iri(FOAF_PERSON));
    assert_eq!(subjects.len(), 1);
    assert_eq!(subjects[0], alice());
}

#[test]
fn predicates_for_subject_object_alice_bob() {
    let g = build_sample_graph();
    let predicates = g.predicates_for_subject_object(&alice(), &bob());
    assert_eq!(predicates.len(), 1);
    assert_eq!(predicates[0], FOAF_KNOWS);
}

#[test]
fn predicates_for_subject_object_no_match() {
    let g = build_sample_graph();
    let predicates = g.predicates_for_subject_object(&bob(), &alice());
    assert!(predicates.is_empty());
}

// ════════════════════════════════════════════════════════════════
//  Combined: typed literals in graph via From conversions
// ════════════════════════════════════════════════════════════════

#[test]
fn graph_with_typed_literal_from_i64() {
    let mut g = RdfGraph::new();
    let age_lit: RdfTerm = Literal::from(30i64).into();
    g.add_triple(alice(), FOAF_AGE, age_lit.clone()).unwrap();

    let objects = g.objects_for_subject_predicate(&alice(), FOAF_AGE);
    assert_eq!(objects.len(), 1);
    let lit = objects[0].as_literal().unwrap();
    assert_eq!(lit.value(), "30");
    assert_eq!(lit.datatype(), XSD_INTEGER);
}

#[test]
fn graph_with_typed_literal_from_bool() {
    let mut g = RdfGraph::new();
    let active: RdfTerm = Literal::from(true).into();
    let pred = "http://example.org/active";
    g.add_triple(alice(), pred, active).unwrap();

    let objects = g.objects_for_subject_predicate(&alice(), pred);
    assert_eq!(objects.len(), 1);
    let lit = objects[0].as_literal().unwrap();
    assert_eq!(lit.value(), "true");
    assert_eq!(lit.datatype(), XSD_BOOLEAN);
}

#[test]
fn graph_query_after_removal() {
    let mut g = build_sample_graph();
    assert_eq!(g.triples_for_subject(&alice()).len(), 3);

    g.remove_triple(&alice(), FOAF_KNOWS, &bob()).unwrap();
    assert_eq!(g.triples_for_subject(&alice()).len(), 2);
    assert!(g.objects_for_subject_predicate(&alice(), FOAF_KNOWS).is_empty());
}

#[test]
fn graph_multiple_objects_for_same_predicate() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
    g.add_triple(alice(), FOAF_KNOWS, charlie()).unwrap();

    let objects = g.objects_for_subject_predicate(&alice(), FOAF_KNOWS);
    assert_eq!(objects.len(), 2);
    assert!(objects.contains(&bob()));
    assert!(objects.contains(&charlie()));
}

#[test]
fn graph_multiple_subjects_for_same_predicate_object() {
    let mut g = RdfGraph::new();
    let person = RdfTerm::iri(FOAF_PERSON);
    g.add_triple(alice(), RDF_TYPE, person.clone()).unwrap();
    g.add_triple(bob(), RDF_TYPE, person.clone()).unwrap();

    let subjects = g.subjects_for_predicate_object(RDF_TYPE, &person);
    assert_eq!(subjects.len(), 2);
    assert!(subjects.contains(&alice()));
    assert!(subjects.contains(&bob()));
}
