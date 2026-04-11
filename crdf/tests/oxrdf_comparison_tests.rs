//! Tests that validate crdf structures against oxrdf to ensure correctness
//! with a well-established RDF library.

use crdf::{Literal, RdfGraph, RdfTerm, Triple};

const EX_ALICE: &str = "http://example.org/alice";
const EX_BOB: &str = "http://example.org/bob";
const FOAF_NAME: &str = "http://xmlns.com/foaf/0.1/name";
const FOAF_KNOWS: &str = "http://xmlns.com/foaf/0.1/knows";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const FOAF_PERSON: &str = "http://xmlns.com/foaf/0.1/Person";
const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";

// ---------------------------------------------------------------------------
// Helper: convert a crdf Triple into an oxrdf Triple for comparison
// ---------------------------------------------------------------------------

fn crdf_term_to_oxrdf_term(term: &RdfTerm) -> oxrdf::Term {
    match term {
        RdfTerm::Iri(iri) => oxrdf::NamedNode::new(iri).unwrap().into(),
        RdfTerm::BlankNode(id) => oxrdf::BlankNode::new(id).unwrap().into(),
        RdfTerm::Literal(lit) => if let Some(ref lang) = lit.language {
            oxrdf::Literal::new_language_tagged_literal(lit.value.as_str(), lang).unwrap()
        } else {
            oxrdf::Literal::new_typed_literal(
                lit.value.as_str(),
                oxrdf::NamedNode::new(lit.datatype.as_str()).unwrap(),
            )
        }
        .into(),
    }
}

fn crdf_subject_to_oxrdf(term: &RdfTerm) -> oxrdf::NamedOrBlankNode {
    match term {
        RdfTerm::Iri(iri) => oxrdf::NamedNode::new(iri).unwrap().into(),
        RdfTerm::BlankNode(id) => oxrdf::BlankNode::new(id).unwrap().into(),
        RdfTerm::Literal(_) => panic!("Literals cannot be subjects in RDF"),
    }
}

fn crdf_triple_to_oxrdf(t: &Triple) -> oxrdf::Triple {
    oxrdf::Triple::new(
        crdf_subject_to_oxrdf(&t.subject),
        oxrdf::NamedNode::new(&t.predicate).unwrap(),
        crdf_term_to_oxrdf_term(&t.object),
    )
}

// ---------------------------------------------------------------------------
// IRI / NamedNode comparison
// ---------------------------------------------------------------------------

#[test]
fn iri_matches_named_node() {
    let crdf_iri = RdfTerm::iri(EX_ALICE);
    let ox_named = oxrdf::NamedNode::new(EX_ALICE).unwrap();

    // Both should produce the same IRI string
    match &crdf_iri {
        RdfTerm::Iri(iri) => assert_eq!(iri.as_str(), ox_named.as_str()),
        _ => panic!("Expected IRI"),
    }

    // Round-trip through conversion
    let converted = crdf_term_to_oxrdf_term(&crdf_iri);
    assert_eq!(converted, oxrdf::Term::from(ox_named));
}

// ---------------------------------------------------------------------------
// BlankNode comparison
// ---------------------------------------------------------------------------

#[test]
fn blank_node_matches() {
    let crdf_bn = RdfTerm::blank_node("b0");
    let ox_bn = oxrdf::BlankNode::new("b0").unwrap();

    match &crdf_bn {
        RdfTerm::BlankNode(id) => assert_eq!(id.as_str(), ox_bn.as_str()),
        _ => panic!("Expected BlankNode"),
    }

    let converted = crdf_term_to_oxrdf_term(&crdf_bn);
    assert_eq!(converted, oxrdf::Term::from(ox_bn));
}

// ---------------------------------------------------------------------------
// Simple literal (no datatype, no language)
// ---------------------------------------------------------------------------

#[test]
fn simple_literal_matches() {
    let crdf_lit = RdfTerm::literal("hello");
    let ox_lit = oxrdf::Literal::new_simple_literal("hello");

    if let RdfTerm::Literal(lit) = &crdf_lit {
        assert_eq!(lit.value.as_str(), ox_lit.value());
        // oxrdf simple literals have xsd:string as datatype
        assert_eq!(
            ox_lit.datatype(),
            oxrdf::NamedNodeRef::new("http://www.w3.org/2001/XMLSchema#string").unwrap()
        );
        // crdf simple literals now also have xsd:string as datatype (RDF 1.1)
        assert_eq!(
            lit.datatype.as_str(),
            "http://www.w3.org/2001/XMLSchema#string"
        );
    } else {
        panic!("Expected Literal");
    }

    let converted = crdf_term_to_oxrdf_term(&crdf_lit);
    assert_eq!(converted, oxrdf::Term::from(ox_lit));
}

// ---------------------------------------------------------------------------
// Typed literal
// ---------------------------------------------------------------------------

#[test]
fn typed_literal_matches() {
    let crdf_lit = RdfTerm::Literal(Literal::new("42").with_datatype(XSD_INTEGER).unwrap());
    let ox_lit =
        oxrdf::Literal::new_typed_literal("42", oxrdf::NamedNode::new(XSD_INTEGER).unwrap());

    if let RdfTerm::Literal(lit) = &crdf_lit {
        assert_eq!(lit.value.as_str(), ox_lit.value());
        assert_eq!(lit.datatype.as_str(), ox_lit.datatype().as_str());
    } else {
        panic!("Expected Literal");
    }

    let converted = crdf_term_to_oxrdf_term(&crdf_lit);
    assert_eq!(converted, oxrdf::Term::from(ox_lit));
}

// ---------------------------------------------------------------------------
// Language-tagged literal
// ---------------------------------------------------------------------------

#[test]
fn language_tagged_literal_matches() {
    let crdf_lit = RdfTerm::Literal(Literal::new("hello").with_language("en").unwrap());
    let ox_lit = oxrdf::Literal::new_language_tagged_literal("hello", "en").unwrap();

    if let RdfTerm::Literal(lit) = &crdf_lit {
        assert_eq!(lit.value.as_str(), ox_lit.value());
        assert_eq!(lit.language.as_deref().unwrap(), ox_lit.language().unwrap());
    } else {
        panic!("Expected Literal");
    }

    let converted = crdf_term_to_oxrdf_term(&crdf_lit);
    assert_eq!(converted, oxrdf::Term::from(ox_lit));
}

// ---------------------------------------------------------------------------
// Triple structure comparison
// ---------------------------------------------------------------------------

#[test]
fn triple_structure_matches_oxrdf() {
    let crdf_triple = Triple::new(RdfTerm::iri(EX_ALICE), FOAF_NAME, RdfTerm::literal("Alice"));

    let ox_triple = oxrdf::Triple::new(
        oxrdf::NamedNode::new(EX_ALICE).unwrap(),
        oxrdf::NamedNode::new(FOAF_NAME).unwrap(),
        oxrdf::Literal::new_simple_literal("Alice"),
    );

    let converted = crdf_triple_to_oxrdf(&crdf_triple);
    assert_eq!(converted.subject, ox_triple.subject);
    assert_eq!(converted.predicate, ox_triple.predicate);
    assert_eq!(converted.object, ox_triple.object);
    assert_eq!(converted, ox_triple);
}

#[test]
fn triple_with_iri_object_matches_oxrdf() {
    let crdf_triple = Triple::new(RdfTerm::iri(EX_ALICE), FOAF_KNOWS, RdfTerm::iri(EX_BOB));

    let ox_triple = oxrdf::Triple::new(
        oxrdf::NamedNode::new(EX_ALICE).unwrap(),
        oxrdf::NamedNode::new(FOAF_KNOWS).unwrap(),
        oxrdf::NamedNode::new(EX_BOB).unwrap(),
    );

    assert_eq!(crdf_triple_to_oxrdf(&crdf_triple), ox_triple);
}

#[test]
fn triple_with_blank_node_subject_matches_oxrdf() {
    let crdf_triple = Triple::new(
        RdfTerm::blank_node("b1"),
        FOAF_NAME,
        RdfTerm::literal("Anonymous"),
    );

    let ox_triple = oxrdf::Triple::new(
        oxrdf::BlankNode::new("b1").unwrap(),
        oxrdf::NamedNode::new(FOAF_NAME).unwrap(),
        oxrdf::Literal::new_simple_literal("Anonymous"),
    );

    assert_eq!(crdf_triple_to_oxrdf(&crdf_triple), ox_triple);
}

// ---------------------------------------------------------------------------
// Graph-level: crdf triples match oxrdf Graph contents
// ---------------------------------------------------------------------------

#[test]
fn graph_triples_match_oxrdf_graph() {
    // Build crdf graph
    let mut crdf_g = RdfGraph::new();
    crdf_g
        .add_triple(RdfTerm::iri(EX_ALICE), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    crdf_g
        .add_triple(RdfTerm::iri(EX_ALICE), FOAF_KNOWS, RdfTerm::iri(EX_BOB))
        .unwrap();
    crdf_g
        .add_triple(RdfTerm::iri(EX_BOB), FOAF_NAME, RdfTerm::literal("Bob"))
        .unwrap();

    // Build equivalent oxrdf graph
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
    ox_g.insert(oxrdf::TripleRef::new(
        oxrdf::NamedNodeRef::new(EX_BOB).unwrap(),
        oxrdf::NamedNodeRef::new(FOAF_NAME).unwrap(),
        oxrdf::LiteralRef::new_simple_literal("Bob"),
    ));

    // Same number of triples
    assert_eq!(crdf_g.len(), ox_g.len());

    // Every crdf triple must exist in the oxrdf graph
    for crdf_t in crdf_g.triples() {
        let ox_t = crdf_triple_to_oxrdf(&crdf_t);
        assert!(
            ox_g.contains(ox_t.as_ref()),
            "oxrdf graph missing triple: {crdf_t}"
        );
    }

    // Every oxrdf triple must have a counterpart in crdf
    for ox_t in ox_g.iter() {
        let subj_str = ox_t.subject.to_string();
        let pred_str = ox_t.predicate.as_str();
        let obj_str = ox_t.object.to_string();

        let found = crdf_g.triples().iter().any(|ct| {
            let ct_ox = crdf_triple_to_oxrdf(ct);
            ct_ox.subject.to_string() == subj_str
                && ct_ox.predicate.as_str() == pred_str
                && ct_ox.object.to_string() == obj_str
        });
        assert!(
            found,
            "crdf graph missing triple: {} <{}> {}",
            subj_str, pred_str, obj_str
        );
    }
}

// ---------------------------------------------------------------------------
// RDF type triple comparison
// ---------------------------------------------------------------------------

#[test]
fn rdf_type_triple_matches_oxrdf() {
    let crdf_triple = Triple::new(RdfTerm::iri(EX_ALICE), RDF_TYPE, RdfTerm::iri(FOAF_PERSON));

    let ox_triple = oxrdf::Triple::new(
        oxrdf::NamedNode::new(EX_ALICE).unwrap(),
        oxrdf::NamedNode::new(RDF_TYPE).unwrap(),
        oxrdf::NamedNode::new(FOAF_PERSON).unwrap(),
    );

    assert_eq!(crdf_triple_to_oxrdf(&crdf_triple), ox_triple);
}

// ---------------------------------------------------------------------------
// N-Triples serialization format compatibility
// ---------------------------------------------------------------------------

#[test]
fn display_format_matches_ntriples_style() {
    // Verify that crdf's Display for RdfTerm produces N-Triples-compatible output
    let iri = RdfTerm::iri(EX_ALICE);
    let ox_nn = oxrdf::NamedNode::new(EX_ALICE).unwrap();
    assert_eq!(iri.to_string(), ox_nn.to_string());

    let bnode = RdfTerm::blank_node("b0");
    let ox_bn = oxrdf::BlankNode::new("b0").unwrap();
    assert_eq!(bnode.to_string(), ox_bn.to_string());

    // Simple literal: oxrdf uses "hello" while crdf uses "hello"
    let lit = RdfTerm::literal("hello");
    let ox_lit = oxrdf::Literal::new_simple_literal("hello");
    assert_eq!(lit.to_string(), format!("\"hello\""));
    assert_eq!(ox_lit.to_string(), format!("\"hello\""));
}

// ---------------------------------------------------------------------------
// Typed literal display
// ---------------------------------------------------------------------------

#[test]
fn typed_literal_display_matches_oxrdf() {
    let crdf_lit = RdfTerm::Literal(Literal::new("42").with_datatype(XSD_INTEGER).unwrap());
    let ox_lit =
        oxrdf::Literal::new_typed_literal("42", oxrdf::NamedNode::new(XSD_INTEGER).unwrap());

    // crdf: "42"^^<http://www.w3.org/2001/XMLSchema#integer>
    // oxrdf: "42"^^<http://www.w3.org/2001/XMLSchema#integer>
    assert_eq!(crdf_lit.to_string(), ox_lit.to_string());
}

// ---------------------------------------------------------------------------
// Language-tagged literal display
// ---------------------------------------------------------------------------

#[test]
fn language_tagged_literal_display_matches_oxrdf() {
    let crdf_lit = RdfTerm::Literal(Literal::new("hello").with_language("en").unwrap());
    let ox_lit = oxrdf::Literal::new_language_tagged_literal("hello", "en").unwrap();

    // crdf: "hello"@en
    // oxrdf: "hello"@en
    assert_eq!(crdf_lit.to_string(), ox_lit.to_string());
}

// ---------------------------------------------------------------------------
// Removal: crdf graph after removal matches oxrdf graph
// ---------------------------------------------------------------------------

#[test]
fn graph_after_removal_matches_oxrdf() {
    let mut crdf_g = RdfGraph::new();
    crdf_g
        .add_triple(RdfTerm::iri(EX_ALICE), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    crdf_g
        .add_triple(RdfTerm::iri(EX_ALICE), FOAF_KNOWS, RdfTerm::iri(EX_BOB))
        .unwrap();

    // Remove one triple
    crdf_g
        .remove_triple(
            &RdfTerm::iri(EX_ALICE),
            FOAF_NAME,
            &RdfTerm::literal("Alice"),
        )
        .unwrap();

    // Build the expected oxrdf graph (only the remaining triple)
    let mut ox_g = oxrdf::Graph::default();
    ox_g.insert(oxrdf::TripleRef::new(
        oxrdf::NamedNodeRef::new(EX_ALICE).unwrap(),
        oxrdf::NamedNodeRef::new(FOAF_KNOWS).unwrap(),
        oxrdf::NamedNodeRef::new(EX_BOB).unwrap(),
    ));

    assert_eq!(crdf_g.len(), ox_g.len());

    for crdf_t in crdf_g.triples() {
        let ox_t = crdf_triple_to_oxrdf(&crdf_t);
        assert!(
            ox_g.contains(ox_t.as_ref()),
            "oxrdf graph missing triple after removal: {crdf_t}"
        );
    }
}

// ---------------------------------------------------------------------------
// Replicated graph matches oxrdf
// ---------------------------------------------------------------------------

#[test]
fn replicated_graph_matches_oxrdf() {
    let mut replica_a = RdfGraph::new();
    let mut replica_b = RdfGraph::new();

    let op1 = replica_a
        .add_triple(RdfTerm::iri(EX_ALICE), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    let op2 = replica_a
        .add_triple(RdfTerm::iri(EX_ALICE), RDF_TYPE, RdfTerm::iri(FOAF_PERSON))
        .unwrap();

    replica_b.apply_downstream(op1).unwrap();
    replica_b.apply_downstream(op2).unwrap();

    // Build expected oxrdf graph
    let mut ox_g = oxrdf::Graph::default();
    ox_g.insert(oxrdf::TripleRef::new(
        oxrdf::NamedNodeRef::new(EX_ALICE).unwrap(),
        oxrdf::NamedNodeRef::new(FOAF_NAME).unwrap(),
        oxrdf::LiteralRef::new_simple_literal("Alice"),
    ));
    ox_g.insert(oxrdf::TripleRef::new(
        oxrdf::NamedNodeRef::new(EX_ALICE).unwrap(),
        oxrdf::NamedNodeRef::new(RDF_TYPE).unwrap(),
        oxrdf::NamedNodeRef::new(FOAF_PERSON).unwrap(),
    ));

    // Both replicas should match the oxrdf graph
    for replica in [&replica_a, &replica_b] {
        assert_eq!(replica.len(), ox_g.len());
        for crdf_t in replica.triples() {
            let ox_t = crdf_triple_to_oxrdf(&crdf_t);
            assert!(
                ox_g.contains(ox_t.as_ref()),
                "Replica missing triple: {crdf_t}"
            );
        }
    }
}
