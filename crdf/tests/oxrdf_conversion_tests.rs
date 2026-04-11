use crdf::{CrdfError, Literal, RdfGraph, RdfTerm, Triple};

const EX_ALICE: &str = "http://example.org/alice";
const EX_BOB: &str = "http://example.org/bob";
const FOAF_NAME: &str = "http://xmlns.com/foaf/0.1/name";
const FOAF_KNOWS: &str = "http://xmlns.com/foaf/0.1/knows";

#[test]
fn term_to_oxrdf_named_node() {
    let term = RdfTerm::iri(EX_ALICE);
    let converted = term.to_oxrdf().unwrap();

    let expected = oxrdf::Term::from(oxrdf::NamedNode::new(EX_ALICE).unwrap());
    assert_eq!(converted, expected);
}

#[test]
fn literal_to_oxrdf_language_tagged() {
    let lit = Literal::new("hello").with_language("EN").unwrap();
    let converted = lit.to_oxrdf().unwrap();

    let expected = oxrdf::Literal::new_language_tagged_literal("hello", "en").unwrap();
    assert_eq!(converted, expected);
}

#[test]
fn triple_to_oxrdf() {
    let triple = Triple::new(RdfTerm::iri(EX_ALICE), FOAF_NAME, RdfTerm::literal("Alice"));

    let converted = triple.to_oxrdf().unwrap();
    let expected = oxrdf::Triple::new(
        oxrdf::NamedNode::new(EX_ALICE).unwrap(),
        oxrdf::NamedNode::new(FOAF_NAME).unwrap(),
        oxrdf::Literal::new_simple_literal("Alice"),
    );

    assert_eq!(converted, expected);
}

#[test]
fn graph_to_oxrdf_graph() {
    let mut g = RdfGraph::new();
    g.add_triple(RdfTerm::iri(EX_ALICE), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.add_triple(RdfTerm::iri(EX_ALICE), FOAF_KNOWS, RdfTerm::iri(EX_BOB))
        .unwrap();

    let converted = g.to_oxrdf().unwrap();

    let expected_name = oxrdf::Triple::new(
        oxrdf::NamedNode::new(EX_ALICE).unwrap(),
        oxrdf::NamedNode::new(FOAF_NAME).unwrap(),
        oxrdf::Literal::new_simple_literal("Alice"),
    );
    let expected_knows = oxrdf::Triple::new(
        oxrdf::NamedNode::new(EX_ALICE).unwrap(),
        oxrdf::NamedNode::new(FOAF_KNOWS).unwrap(),
        oxrdf::NamedNode::new(EX_BOB).unwrap(),
    );

    assert_eq!(converted.len(), 2);
    assert!(converted.contains(expected_name.as_ref()));
    assert!(converted.contains(expected_knows.as_ref()));
}

#[test]
fn subject_conversion_rejects_literal() {
    let term = RdfTerm::literal("not-a-subject");
    let err = term.to_oxrdf_subject().unwrap_err();
    assert!(matches!(err, CrdfError::LiteralSubject));
}

// ---------------------------------------------------------------
// oxrdf → crdf conversions
// ---------------------------------------------------------------

#[test]
fn oxrdf_named_node_to_rdf_term() {
    let nn = oxrdf::NamedNode::new(EX_ALICE).unwrap();
    let term: RdfTerm = nn.into();
    assert_eq!(term, RdfTerm::iri(EX_ALICE));
}

#[test]
fn oxrdf_blank_node_to_rdf_term() {
    let bn = oxrdf::BlankNode::new("b0").unwrap();
    let term: RdfTerm = bn.into();
    assert_eq!(term, RdfTerm::blank_node("b0"));
}

#[test]
fn oxrdf_simple_literal_to_literal() {
    let ox = oxrdf::Literal::new_simple_literal("hello");
    let lit: Literal = ox.into();
    assert_eq!(lit.value(), "hello");
    assert_eq!(lit.datatype(), crdf::XSD_STRING);
    assert!(lit.language().is_none());
}

#[test]
fn oxrdf_typed_literal_to_literal() {
    let ox =
        oxrdf::Literal::new_typed_literal("42", oxrdf::NamedNode::new(crdf::XSD_INTEGER).unwrap());
    let lit: Literal = ox.into();
    assert_eq!(lit.value(), "42");
    assert_eq!(lit.datatype(), crdf::XSD_INTEGER);
}

#[test]
fn oxrdf_language_tagged_literal_to_literal() {
    let ox = oxrdf::Literal::new_language_tagged_literal("hello", "en").unwrap();
    let lit: Literal = ox.into();
    assert_eq!(lit.value(), "hello");
    assert_eq!(lit.language(), Some("en"));
    assert_eq!(lit.datatype(), crdf::RDF_LANG_STRING);
}

#[test]
fn oxrdf_term_to_rdf_term() {
    let ox_term = oxrdf::Term::from(oxrdf::NamedNode::new(EX_BOB).unwrap());
    let term: RdfTerm = ox_term.into();
    assert_eq!(term, RdfTerm::iri(EX_BOB));
}

#[test]
fn oxrdf_triple_to_triple() {
    let ox = oxrdf::Triple::new(
        oxrdf::NamedNode::new(EX_ALICE).unwrap(),
        oxrdf::NamedNode::new(FOAF_NAME).unwrap(),
        oxrdf::Literal::new_simple_literal("Alice"),
    );
    let triple: Triple = ox.into();
    assert_eq!(triple.subject, RdfTerm::iri(EX_ALICE));
    assert_eq!(triple.predicate, FOAF_NAME);
    assert_eq!(triple.object, RdfTerm::literal("Alice"));
}

#[test]
fn oxrdf_graph_to_rdf_graph() {
    let mut ox_g = oxrdf::Graph::default();
    ox_g.insert(oxrdf::TripleRef::new(
        oxrdf::NamedNodeRef::new(EX_ALICE).unwrap(),
        oxrdf::NamedNodeRef::new(FOAF_NAME).unwrap(),
        oxrdf::LiteralRef::new_simple_literal("Alice"),
    ));
    ox_g.insert(oxrdf::TripleRef::new(
        oxrdf::NamedNodeRef::new(EX_ALICE).unwrap(),
        oxrdf::NamedNodeRef::new(FOAF_KNOWS).unwrap(),
        oxrdf::NamedNodeRef::new(EX_BOB).unwrap(),
    ));

    let g = RdfGraph::from_oxrdf(&ox_g).unwrap();
    assert_eq!(g.len(), 2);
    assert!(g.contains_triple(
        &RdfTerm::iri(EX_ALICE),
        FOAF_NAME,
        &RdfTerm::literal("Alice")
    ));
    assert!(g.contains_triple(&RdfTerm::iri(EX_ALICE), FOAF_KNOWS, &RdfTerm::iri(EX_BOB)));
}

#[test]
fn roundtrip_crdf_to_oxrdf_and_back() {
    let mut original = RdfGraph::new();
    original
        .add_triple(RdfTerm::iri(EX_ALICE), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    original
        .add_triple(RdfTerm::iri(EX_ALICE), FOAF_KNOWS, RdfTerm::iri(EX_BOB))
        .unwrap();

    let ox_graph = original.to_oxrdf().unwrap();
    let restored = RdfGraph::from_oxrdf(&ox_graph).unwrap();

    assert_eq!(restored.len(), original.len());
    for t in original.triples() {
        assert!(restored.contains_triple(&t.subject, &t.predicate, &t.object));
    }
}
