//! Edge-case tests for the crdf crate.

use crdf::{Literal, RdfGraph, RdfTerm};

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

// ---------------------------------------------------------------------------
// Literal subject rejection
// ---------------------------------------------------------------------------

#[test]
fn literal_with_datatype_as_subject_rejected() {
    let mut g = RdfGraph::new();
    let lit = RdfTerm::Literal(
        Literal::new("42")
            .with_datatype("http://www.w3.org/2001/XMLSchema#integer")
            .unwrap(),
    );
    assert!(g.add_triple(lit, FOAF_NAME, alice()).is_err());
}

#[test]
fn literal_with_language_as_subject_rejected() {
    let mut g = RdfGraph::new();
    let lit = RdfTerm::Literal(Literal::new("hello").with_language("en").unwrap());
    assert!(g.add_triple(lit, FOAF_NAME, alice()).is_err());
}

// ---------------------------------------------------------------------------
// Removing a non-existent triple
// ---------------------------------------------------------------------------

#[test]
fn remove_nonexistent_triple_errors() {
    let mut g = RdfGraph::new();
    let result = g.remove_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice"));
    assert!(result.is_err());
}

#[test]
fn remove_with_unknown_subject_errors() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    // bob was never added as a subject
    let result = g.remove_triple(&bob(), FOAF_NAME, &RdfTerm::literal("Alice"));
    assert!(result.is_err());
}

#[test]
fn remove_with_unknown_object_errors() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    // "Bob" literal was never added as an object
    let result = g.remove_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Bob"));
    assert!(result.is_err());
}

#[test]
fn remove_with_wrong_predicate_errors() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    let result = g.remove_triple(&alice(), FOAF_KNOWS, &RdfTerm::literal("Alice"));
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Double removal
// ---------------------------------------------------------------------------

#[test]
fn remove_same_triple_twice_errors() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.remove_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice"))
        .unwrap();
    // Second removal should fail
    let result = g.remove_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice"));
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Duplicate triple addition (same subject, predicate, object)
// ---------------------------------------------------------------------------

#[test]
fn duplicate_triple_creates_separate_edges() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    // CRDT graph creates separate edges for each add, so both should exist
    assert_eq!(g.len(), 2);
}

// ---------------------------------------------------------------------------
// Literals as objects are valid
// ---------------------------------------------------------------------------

#[test]
fn literal_object_with_datatype() {
    let mut g = RdfGraph::new();
    let lit = RdfTerm::Literal(
        Literal::new("42")
            .with_datatype("http://www.w3.org/2001/XMLSchema#integer")
            .unwrap(),
    );
    g.add_triple(alice(), "http://example.org/age", lit.clone())
        .unwrap();
    assert!(g.contains_triple(&alice(), "http://example.org/age", &lit));
}

#[test]
fn literal_object_with_language_tag() {
    let mut g = RdfGraph::new();
    let lit = RdfTerm::Literal(Literal::new("Alice").with_language("en").unwrap());
    g.add_triple(alice(), FOAF_NAME, lit.clone()).unwrap();
    assert!(g.contains_triple(&alice(), FOAF_NAME, &lit));
}

// ---------------------------------------------------------------------------
// Blank node as object
// ---------------------------------------------------------------------------

#[test]
fn blank_node_as_object() {
    let mut g = RdfGraph::new();
    let bnode = RdfTerm::blank_node("addr1");
    g.add_triple(alice(), "http://example.org/address", bnode.clone())
        .unwrap();
    assert!(g.contains_triple(&alice(), "http://example.org/address", &bnode));
}

// ---------------------------------------------------------------------------
// Distinction between similar-looking terms
// ---------------------------------------------------------------------------

#[test]
fn iri_and_literal_with_same_value_are_distinct() {
    let mut g = RdfGraph::new();
    let iri = RdfTerm::iri("http://example.org/alice");
    let lit = RdfTerm::literal("http://example.org/alice");

    g.add_triple(alice(), FOAF_NAME, iri.clone()).unwrap();
    g.add_triple(alice(), FOAF_NAME, lit.clone()).unwrap();

    assert_eq!(g.len(), 2);
    assert!(g.contains_triple(&alice(), FOAF_NAME, &iri));
    assert!(g.contains_triple(&alice(), FOAF_NAME, &lit));
}

#[test]
fn blank_node_and_iri_with_same_value_are_distinct() {
    let mut g = RdfGraph::new();
    let iri = RdfTerm::iri("http://example.org/b0");
    let bnode = RdfTerm::blank_node("http://example.org/b0");

    g.add_triple(iri.clone(), FOAF_NAME, RdfTerm::literal("A"))
        .unwrap();
    g.add_triple(bnode.clone(), FOAF_NAME, RdfTerm::literal("B"))
        .unwrap();

    assert_eq!(g.len(), 2);
    assert!(g.contains_triple(&iri, FOAF_NAME, &RdfTerm::literal("A")));
    assert!(g.contains_triple(&bnode, FOAF_NAME, &RdfTerm::literal("B")));
}

#[test]
fn plain_literal_vs_typed_literal_are_distinct() {
    let mut g = RdfGraph::new();
    let plain = RdfTerm::literal("42");
    let typed = RdfTerm::Literal(
        Literal::new("42")
            .with_datatype("http://www.w3.org/2001/XMLSchema#integer")
            .unwrap(),
    );

    g.add_triple(alice(), "http://example.org/value", plain.clone())
        .unwrap();
    g.add_triple(alice(), "http://example.org/value", typed.clone())
        .unwrap();

    assert_eq!(g.len(), 2);
    assert!(g.contains_triple(&alice(), "http://example.org/value", &plain));
    assert!(g.contains_triple(&alice(), "http://example.org/value", &typed));
}

#[test]
fn plain_literal_vs_language_tagged_literal_are_distinct() {
    let mut g = RdfGraph::new();
    let plain = RdfTerm::literal("hello");
    let tagged = RdfTerm::Literal(Literal::new("hello").with_language("en").unwrap());

    g.add_triple(alice(), FOAF_NAME, plain.clone()).unwrap();
    g.add_triple(alice(), FOAF_NAME, tagged.clone()).unwrap();

    assert_eq!(g.len(), 2);
    assert!(g.contains_triple(&alice(), FOAF_NAME, &plain));
    assert!(g.contains_triple(&alice(), FOAF_NAME, &tagged));
}

#[test]
fn different_language_tags_are_distinct() {
    let mut g = RdfGraph::new();
    let en = RdfTerm::Literal(Literal::new("hello").with_language("en").unwrap());
    let fr = RdfTerm::Literal(Literal::new("hello").with_language("fr").unwrap());

    g.add_triple(alice(), FOAF_NAME, en.clone()).unwrap();
    g.add_triple(alice(), FOAF_NAME, fr.clone()).unwrap();

    assert_eq!(g.len(), 2);
    assert!(g.contains_triple(&alice(), FOAF_NAME, &en));
    assert!(g.contains_triple(&alice(), FOAF_NAME, &fr));
}

// ---------------------------------------------------------------------------
// Empty string values
// ---------------------------------------------------------------------------

#[test]
fn empty_string_literal() {
    let mut g = RdfGraph::new();
    let lit = RdfTerm::literal("");
    g.add_triple(alice(), FOAF_NAME, lit.clone()).unwrap();
    assert!(g.contains_triple(&alice(), FOAF_NAME, &lit));
    assert_eq!(g.len(), 1);
}

// ---------------------------------------------------------------------------
// Unicode content
// ---------------------------------------------------------------------------

#[test]
fn unicode_literal_value() {
    let mut g = RdfGraph::new();
    let lit = RdfTerm::literal("こんにちは世界 🌍");
    g.add_triple(alice(), FOAF_NAME, lit.clone()).unwrap();
    assert!(g.contains_triple(&alice(), FOAF_NAME, &lit));
}

#[test]
fn unicode_iri() {
    let mut g = RdfGraph::new();
    let subj = RdfTerm::iri("http://example.org/資源");
    let pred = "http://example.org/名前";
    let obj = RdfTerm::literal("テスト");
    g.add_triple(subj.clone(), pred, obj.clone()).unwrap();
    assert!(g.contains_triple(&subj, pred, &obj));
}

// ---------------------------------------------------------------------------
// Self-referencing triple (subject == object)
// ---------------------------------------------------------------------------

#[test]
fn self_referencing_triple() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_KNOWS, alice()).unwrap();
    assert_eq!(g.len(), 1);
    assert!(g.contains_triple(&alice(), FOAF_KNOWS, &alice()));
}

// ---------------------------------------------------------------------------
// Large graph
// ---------------------------------------------------------------------------

#[test]
fn many_triples() {
    let mut g = RdfGraph::new();
    for i in 0..100 {
        g.add_triple(
            alice(),
            &format!("http://example.org/prop{i}"),
            RdfTerm::literal(format!("value{i}")),
        )
        .unwrap();
    }
    assert_eq!(g.len(), 100);
}

// ---------------------------------------------------------------------------
// triples_matching edge cases
// ---------------------------------------------------------------------------

#[test]
fn triples_matching_all_none_returns_all() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.add_triple(bob(), FOAF_NAME, RdfTerm::literal("Bob"))
        .unwrap();

    let all = g.triples_matching(None, None, None);
    assert_eq!(all.len(), 2);
}

#[test]
fn triples_matching_no_match_returns_empty() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();

    let result = g.triples_matching(Some(&bob()), None, None);
    assert!(result.is_empty());
}

#[test]
fn triples_matching_by_object() {
    let mut g = RdfGraph::new();
    let obj = RdfTerm::literal("Alice");
    g.add_triple(alice(), FOAF_NAME, obj.clone()).unwrap();
    g.add_triple(bob(), FOAF_NAME, RdfTerm::literal("Bob"))
        .unwrap();

    let found = g.triples_matching(None, None, Some(&obj));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].subject, alice());
}

#[test]
fn triples_matching_all_three_filters() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
    g.add_triple(bob(), FOAF_NAME, RdfTerm::literal("Bob"))
        .unwrap();

    let found = g.triples_matching(
        Some(&alice()),
        Some(FOAF_NAME),
        Some(&RdfTerm::literal("Alice")),
    );
    assert_eq!(found.len(), 1);
}

// ---------------------------------------------------------------------------
// contains_triple on empty graph
// ---------------------------------------------------------------------------

#[test]
fn contains_triple_empty_graph() {
    let g = RdfGraph::new();
    assert!(!g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
}

// ---------------------------------------------------------------------------
// subjects / predicates / objects on empty graph
// ---------------------------------------------------------------------------

#[test]
fn subjects_empty_graph() {
    let g = RdfGraph::new();
    assert!(g.subjects().is_empty());
}

#[test]
fn predicates_empty_graph() {
    let g = RdfGraph::new();
    assert!(g.predicates().is_empty());
}

#[test]
fn objects_empty_graph() {
    let g = RdfGraph::new();
    assert!(g.objects().is_empty());
}

// ---------------------------------------------------------------------------
// Replica: concurrent add + remove (add-wins semantics)
// ---------------------------------------------------------------------------

#[test]
fn concurrent_add_and_remove_add_wins() {
    let mut replica_a = RdfGraph::new();
    let mut replica_b = RdfGraph::new();

    // Both start with the same triple
    let op_add = replica_a
        .add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    replica_b.apply_downstream(op_add).unwrap();

    // A removes it, B concurrently adds another triple
    let op_remove = replica_a
        .remove_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice"))
        .unwrap();
    let op_add2 = replica_b.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();

    // Exchange
    replica_b.apply_downstream(op_remove).unwrap();
    replica_a.apply_downstream(op_add2).unwrap();

    // Both should agree: the removed triple is gone, the new one exists
    assert_eq!(replica_a.len(), replica_b.len());
    assert!(replica_a.contains_triple(&alice(), FOAF_KNOWS, &bob()));
    assert!(replica_b.contains_triple(&alice(), FOAF_KNOWS, &bob()));
    assert!(!replica_a.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
    assert!(!replica_b.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
}

// ---------------------------------------------------------------------------
// Replica: apply same add operation twice (idempotency of downstream)
// ---------------------------------------------------------------------------

#[test]
fn apply_downstream_add_twice() {
    let mut replica_a = RdfGraph::new();
    let mut replica_b = RdfGraph::new();

    let op = replica_a
        .add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();

    replica_b.apply_downstream(op.clone()).unwrap();
    // Applying the same op again should not panic (CRDT idempotency)
    // It may error or succeed depending on the CRDT implementation,
    // but the graph state should remain consistent.
    let _ = replica_b.apply_downstream(op);

    // At least the original triple exists
    assert!(replica_b.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
}

// ---------------------------------------------------------------------------
// Replica: three replicas converge
// ---------------------------------------------------------------------------

#[test]
fn three_replicas_converge_distinct_subjects() {
    // Each replica adds triples about distinct subjects to avoid
    // the duplicate-vertex-UUID issue when the same term is created
    // independently on multiple replicas.
    let mut r1 = RdfGraph::new();
    let mut r2 = RdfGraph::new();
    let mut r3 = RdfGraph::new();

    let charlie = RdfTerm::iri("http://example.org/charlie");

    let op1 = r1
        .add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    let op2 = r2
        .add_triple(bob(), FOAF_NAME, RdfTerm::literal("Bob"))
        .unwrap();
    let op3 = r3
        .add_triple(charlie.clone(), FOAF_NAME, RdfTerm::literal("Charlie"))
        .unwrap();

    // Broadcast all ops to all replicas
    for (replica, ops) in [
        (&mut r1, vec![op2.clone(), op3.clone()]),
        (&mut r2, vec![op1.clone(), op3]),
        (&mut r3, vec![op1, op2]),
    ] {
        for op in ops {
            replica.apply_downstream(op).unwrap();
        }
    }

    // All three should have 3 triples
    assert_eq!(r1.len(), 3);
    assert_eq!(r2.len(), 3);
    assert_eq!(r3.len(), 3);

    // Verify the same triples on every replica
    for r in [&r1, &r2, &r3] {
        assert!(r.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
        assert!(r.contains_triple(&bob(), FOAF_NAME, &RdfTerm::literal("Bob")));
        assert!(r.contains_triple(&charlie, FOAF_NAME, &RdfTerm::literal("Charlie")));
    }
}

/// When the same term (alice) is created independently on two replicas
/// and then synced, term_to_vertex tracks all UUIDs for alice so both
/// replicas can find all triples after convergence.
#[test]
fn concurrent_same_term_creation_converges() {
    let mut r1 = RdfGraph::new();
    let mut r2 = RdfGraph::new();

    // Both replicas independently create alice as a subject
    let op1 = r1
        .add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    let op2 = r2
        .add_triple(alice(), RDF_TYPE, RdfTerm::iri(FOAF_PERSON))
        .unwrap();

    // Exchange operations
    r1.apply_downstream(op2).unwrap();
    r2.apply_downstream(op1).unwrap();

    // Both replicas have 2 edges at the CRDT level
    assert_eq!(r1.len(), 2);
    assert_eq!(r2.len(), 2);

    // Both replicas can now find all triples
    assert!(r1.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
    assert!(r1.contains_triple(&alice(), RDF_TYPE, &RdfTerm::iri(FOAF_PERSON)));
    assert!(r2.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
    assert!(r2.contains_triple(&alice(), RDF_TYPE, &RdfTerm::iri(FOAF_PERSON)));
}

// ---------------------------------------------------------------------------
// Graph state after add-remove-add cycle
// ---------------------------------------------------------------------------

#[test]
fn add_remove_readd_triple() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.remove_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice"))
        .unwrap();
    assert_eq!(g.len(), 0);

    // Re-add the same triple
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    assert_eq!(g.len(), 1);
    assert!(g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
}

// ---------------------------------------------------------------------------
// Long IRI / literal values
// ---------------------------------------------------------------------------

#[test]
fn very_long_iri() {
    let mut g = RdfGraph::new();
    let long_iri = format!("http://example.org/{}", "a".repeat(10_000));
    let subj = RdfTerm::iri(&long_iri);
    g.add_triple(subj.clone(), FOAF_NAME, RdfTerm::literal("test"))
        .unwrap();
    assert!(g.contains_triple(&subj, FOAF_NAME, &RdfTerm::literal("test")));
}

#[test]
fn very_long_literal() {
    let mut g = RdfGraph::new();
    let long_val = "x".repeat(100_000);
    let lit = RdfTerm::literal(&long_val);
    g.add_triple(alice(), FOAF_NAME, lit.clone()).unwrap();
    assert!(g.contains_triple(&alice(), FOAF_NAME, &lit));
}

// ---------------------------------------------------------------------------
// Special characters in literals
// ---------------------------------------------------------------------------

#[test]
fn literal_with_quotes_and_newlines() {
    let mut g = RdfGraph::new();
    let lit = RdfTerm::literal("line1\nline2\t\"quoted\"");
    g.add_triple(alice(), FOAF_NAME, lit.clone()).unwrap();
    assert!(g.contains_triple(&alice(), FOAF_NAME, &lit));
}

#[test]
fn literal_with_backslashes() {
    let mut g = RdfGraph::new();
    let lit = RdfTerm::literal("C:\\Users\\test\\path");
    g.add_triple(alice(), FOAF_NAME, lit.clone()).unwrap();
    assert!(g.contains_triple(&alice(), FOAF_NAME, &lit));
}
