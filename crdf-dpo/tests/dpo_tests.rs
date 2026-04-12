use crdf::{RdfGraph, RdfTerm};
use crdf_dpo::{DpoError, DpoRule, PatternPredicate, PatternTerm, PatternTriple};

const EX: &str = "http://example.org/";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

fn ex(local: &str) -> String {
    format!("{}{}", EX, local)
}

fn ex_iri(local: &str) -> RdfTerm {
    RdfTerm::iri(ex(local))
}

/// Single person graph: Alice livesIn Tokyo, Alice a Person.
/// Tokyo has no other incident triples, so dangling condition is satisfied.
fn build_single_person_graph() -> RdfGraph {
    let mut g = RdfGraph::new();
    g.add_triple(ex_iri("Alice"), ex("livesIn"), ex_iri("Tokyo"))
        .unwrap();
    g.add_triple(ex_iri("Alice"), RDF_TYPE, ex_iri("Person"))
        .unwrap();
    g
}

// ========================================================
// 空グラフ・空ルール
// ========================================================

#[test]
fn test_empty_l_k_r() {
    // L=K=R=empty → valid but no match in any graph
    let rule = DpoRule::new("empty", vec![], vec![], vec![]);
    assert!(rule.validate().is_ok());

    let mut graph = build_single_person_graph();
    // Empty pattern matches once (trivially) → 0 ops
    let result = rule.apply(&mut graph).unwrap();
    assert!(result.operations.is_empty());
}

// ========================================================
// 具体項のみのパターン（変数なし）
// ========================================================

#[test]
fn test_all_concrete_pattern_match() {
    let mut graph = RdfGraph::new();
    graph
        .add_triple(ex_iri("Alice"), ex("livesIn"), ex_iri("Tokyo"))
        .unwrap();

    let rule = DpoRule::new(
        "concrete_delete",
        vec![PatternTriple::new(
            PatternTerm::concrete(ex_iri("Alice")),
            PatternPredicate::concrete(ex("livesIn")),
            PatternTerm::concrete(ex_iri("Tokyo")),
        )],
        vec![],
        vec![],
    );

    rule.apply(&mut graph).unwrap();
    assert_eq!(graph.len(), 0);
}

#[test]
fn test_all_concrete_pattern_no_match() {
    let mut graph = RdfGraph::new();
    graph
        .add_triple(ex_iri("Alice"), ex("livesIn"), ex_iri("Tokyo"))
        .unwrap();

    let rule = DpoRule::new(
        "concrete_no_match",
        vec![PatternTriple::new(
            PatternTerm::concrete(ex_iri("Bob")),
            PatternPredicate::concrete(ex("livesIn")),
            PatternTerm::concrete(ex_iri("Tokyo")),
        )],
        vec![],
        vec![],
    );

    let result = rule.apply(&mut graph);
    assert!(matches!(result, Err(DpoError::NoMatchFound)));
}

// ========================================================
// リテラル・BlankNode のマッチ
// ========================================================

#[test]
fn test_literal_object_matching() {
    let mut graph = RdfGraph::new();
    graph
        .add_triple(ex_iri("Alice"), ex("age"), RdfTerm::literal("30"))
        .unwrap();
    graph
        .add_triple(ex_iri("Alice"), RDF_TYPE, ex_iri("Person"))
        .unwrap();

    let person_type = PatternTriple::new(
        PatternTerm::concrete(ex_iri("Alice")),
        PatternPredicate::concrete(RDF_TYPE),
        PatternTerm::concrete(ex_iri("Person")),
    );

    // Replace age "30" with "31"
    let rule = DpoRule::new(
        "birthday",
        vec![
            PatternTriple::new(
                PatternTerm::concrete(ex_iri("Alice")),
                PatternPredicate::concrete(ex("age")),
                PatternTerm::concrete(RdfTerm::literal("30")),
            ),
            person_type.clone(),
        ],
        vec![person_type.clone()],
        vec![
            person_type,
            PatternTriple::new(
                PatternTerm::concrete(ex_iri("Alice")),
                PatternPredicate::concrete(ex("age")),
                PatternTerm::concrete(RdfTerm::literal("31")),
            ),
        ],
    );

    rule.apply(&mut graph).unwrap();
    assert!(graph.contains_triple(&ex_iri("Alice"), &ex("age"), &RdfTerm::literal("31")));
    assert!(!graph.contains_triple(&ex_iri("Alice"), &ex("age"), &RdfTerm::literal("30")));
}

// ========================================================
// Dangling condition test with concrete nodes
// ========================================================

fn eiri(local: &str) -> PatternTerm {
    PatternTerm::concrete(ex_iri(local))
}

fn epred(local: &str) -> PatternPredicate {
    PatternPredicate::concrete(ex(local))
}

/// Without explicit interface nodes, dangling condition blocks deleting
/// an edge when the subject has other incident triples.
#[test]
fn test_dangling_condition_without_explicit_nodes() {
    let mut graph = RdfGraph::new();
    graph
        .add_triple(ex_iri("A"), ex("knows"), ex_iri("B"))
        .unwrap();
    graph
        .add_triple(ex_iri("A"), ex("likes"), ex_iri("C"))
        .unwrap();

    // L: (A knows B), K: empty, R: empty
    // Node A appears in L\K but not in K → considered removed.
    // But A is incident to (A likes C) which is NOT in L → dangling violation!
    let rule = DpoRule::new(
        "delete_edge_no_preserve",
        vec![PatternTriple::new(eiri("A"), epred("knows"), eiri("B"))],
        vec![],
        vec![],
    );

    let result = rule.apply(&mut graph);
    assert!(
        matches!(result, Err(DpoError::DanglingCondition(_))),
        "Expected dangling condition violation, got: {:?}",
        result
    );
}
