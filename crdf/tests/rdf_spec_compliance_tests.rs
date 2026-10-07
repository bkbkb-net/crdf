//! Tests for RDF 1.1 specification compliance.
//!
//! Reference: <https://www.w3.org/TR/rdf11-concepts/>

use std::collections::HashSet;

use crdf::{
    CrdfError, Literal, RDF_LANG_STRING, RdfGraph, RdfTerm, Triple, XSD_INTEGER, XSD_STRING,
};

const FOAF_NAME: &str = "http://xmlns.com/foaf/0.1/name";
const FOAF_KNOWS: &str = "http://xmlns.com/foaf/0.1/knows";
const EX_AGE: &str = "http://example.org/age";

fn alice() -> RdfTerm {
    RdfTerm::iri("http://example.org/alice")
}

fn bob() -> RdfTerm {
    RdfTerm::iri("http://example.org/bob")
}

// ════════════════════════════════════════════════════════════════════════════
//  §3: "An RDF graph is a set of RDF triples" — duplicate triple handling
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn duplicate_triple_triples_returns_both_crdt_edges() {
    // CRDT semantics: each add_triple creates a unique edge.
    // This documents the intentional deviation from RDF set semantics.
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();

    // Underlying CRDT edge count is 2
    assert_eq!(g.len(), 2);
    // triples() returns both (CRDT-level view)
    assert_eq!(g.triples().len(), 2);
}

#[test]
fn duplicate_triple_contains_still_finds_it() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    assert!(g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
}

#[test]
fn duplicate_triple_remove_one_still_has_other() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    assert_eq!(g.len(), 2);

    g.remove_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice"))
        .unwrap();
    // One edge removed, but the other remains (CRDT add-wins)
    assert_eq!(g.len(), 1);
    assert!(g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
}

// ════════════════════════════════════════════════════════════════════════════
//  §3.3: Language tag — must be non-empty (BCP47)
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn empty_language_tag_returns_error() {
    let result = Literal::new("hello").with_language("");
    assert!(matches!(result, Err(CrdfError::EmptyLanguageTag)));
}

#[test]
fn non_empty_language_tag_accepted() {
    let lit = Literal::new("hello").with_language("en").unwrap();
    assert_eq!(lit.language(), Some("en"));
    assert_eq!(lit.datatype(), RDF_LANG_STRING);
}

// ════════════════════════════════════════════════════════════════════════════
//  §3.3: rdf:langString datatype requires a language tag ("if and only if")
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn lang_string_datatype_without_language_returns_error() {
    let result = Literal::new("hello").with_datatype(RDF_LANG_STRING);
    assert!(matches!(result, Err(CrdfError::LangStringDatatype)));
}

#[test]
fn with_language_sets_lang_string_datatype() {
    let lit = Literal::new("hello").with_language("de").unwrap();
    assert_eq!(lit.datatype(), RDF_LANG_STRING);
    assert_eq!(lit.language(), Some("de"));
}

#[test]
fn with_datatype_clears_language() {
    let lit = Literal::new("42")
        .with_language("en")
        .unwrap()
        .with_datatype(XSD_INTEGER)
        .unwrap();
    assert_eq!(lit.datatype(), XSD_INTEGER);
    assert_eq!(lit.language(), None);
}

// ════════════════════════════════════════════════════════════════════════════
//  §3.3: Language tag normalization — "value space is always in lower case"
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn language_tag_normalized_to_lowercase() {
    let lit = Literal::new("hello").with_language("EN").unwrap();
    assert_eq!(lit.language(), Some("en"));
}

#[test]
fn mixed_case_language_tag_normalized() {
    let lit = Literal::new("hello").with_language("en-US").unwrap();
    assert_eq!(lit.language(), Some("en-us"));
}

#[test]
fn language_tag_case_insensitive_equality() {
    // "hello"@en and "hello"@EN should be the same literal after normalization
    let lit_lower = Literal::new("hello").with_language("en").unwrap();
    let lit_upper = Literal::new("hello").with_language("EN").unwrap();
    assert_eq!(lit_lower, lit_upper);
}

#[test]
fn language_tag_case_insensitive_in_graph() {
    let mut g = RdfGraph::new();
    let en_lower = RdfTerm::Literal(Literal::new("hello").with_language("en").unwrap());
    let en_upper = RdfTerm::Literal(Literal::new("hello").with_language("EN").unwrap());

    g.add_triple(alice(), FOAF_NAME, en_lower).unwrap();

    // After normalization, both should match the same term
    assert!(g.contains_triple(&alice(), FOAF_NAME, &en_upper));
}

// ════════════════════════════════════════════════════════════════════════════
//  subjects() / objects() uniqueness
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn subjects_returns_unique_terms() {
    let mut g = RdfGraph::new();
    // alice appears as subject in multiple triples
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
    g.add_triple(alice(), EX_AGE, RdfTerm::Literal(Literal::from(30i64)))
        .unwrap();
    g.add_triple(bob(), FOAF_NAME, RdfTerm::literal("Bob"))
        .unwrap();

    let subjects = g.subjects();
    let unique: HashSet<_> = subjects.iter().collect();
    assert_eq!(
        subjects.len(),
        unique.len(),
        "subjects() must not contain duplicates"
    );
    assert_eq!(subjects.len(), 2); // alice and bob
}

#[test]
fn objects_returns_unique_terms() {
    let mut g = RdfGraph::new();
    let alice_lit = RdfTerm::literal("Alice");
    // alice_lit appears as object in two triples with different predicates
    g.add_triple(alice(), FOAF_NAME, alice_lit.clone()).unwrap();
    g.add_triple(bob(), FOAF_NAME, alice_lit.clone()).unwrap();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();

    let objects = g.objects();
    let unique: HashSet<_> = objects.iter().collect();
    assert_eq!(
        objects.len(),
        unique.len(),
        "objects() must not contain duplicates"
    );
    // "Alice" and bob()
    assert_eq!(objects.len(), 2);
}

#[test]
fn subjects_unique_even_with_non_adjacent_edges() {
    let mut g = RdfGraph::new();
    // Create interleaved pattern: alice, bob, alice
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    g.add_triple(bob(), FOAF_NAME, RdfTerm::literal("Bob"))
        .unwrap();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();

    let subjects = g.subjects();
    let unique: HashSet<_> = subjects.iter().collect();
    assert_eq!(subjects.len(), unique.len());
}

#[test]
fn objects_unique_even_with_non_adjacent_edges() {
    let mut g = RdfGraph::new();
    let lit = RdfTerm::literal("same");
    // Pattern: same object, different object, same object
    g.add_triple(alice(), FOAF_NAME, lit.clone()).unwrap();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
    g.add_triple(bob(), FOAF_NAME, lit.clone()).unwrap();

    let objects = g.objects();
    let unique: HashSet<_> = objects.iter().collect();
    assert_eq!(objects.len(), unique.len());
}

// ════════════════════════════════════════════════════════════════════════════
//  Literal Display — N-Triples escaping
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn display_escapes_double_quote() {
    let lit = Literal::new("say \"hi\"");
    assert_eq!(lit.to_string(), r#""say \"hi\"""#);
}

#[test]
fn display_escapes_backslash() {
    let lit = Literal::new(r"path\to\file");
    assert_eq!(lit.to_string(), r#""path\\to\\file""#);
}

#[test]
fn display_escapes_newline() {
    let lit = Literal::new("line1\nline2");
    assert_eq!(lit.to_string(), r#""line1\nline2""#);
}

#[test]
fn display_escapes_carriage_return() {
    let lit = Literal::new("line1\rline2");
    assert_eq!(lit.to_string(), r#""line1\rline2""#);
}

#[test]
fn display_escapes_tab() {
    let lit = Literal::new("col1\tcol2");
    assert_eq!(lit.to_string(), r#""col1\tcol2""#);
}

#[test]
fn display_escapes_combined_special_chars() {
    let lit = Literal::new("a\"b\\c\nd\te");
    assert_eq!(lit.to_string(), r#""a\"b\\c\nd\te""#);
}

#[test]
fn display_typed_literal_escapes_value() {
    let lit = Literal::new("say \"hi\"")
        .with_datatype(XSD_STRING)
        .unwrap();
    // xsd:string omitted in display, but value must be escaped
    assert_eq!(lit.to_string(), r#""say \"hi\"""#);
}

#[test]
fn display_language_tagged_literal_escapes_value() {
    let lit = Literal::new("say \"hi\"").with_language("en").unwrap();
    assert_eq!(lit.to_string(), r#""say \"hi\""@en"#);
}

#[test]
fn rdf_term_display_escapes_literal() {
    let term = RdfTerm::Literal(Literal::new("line1\nline2"));
    assert_eq!(term.to_string(), r#""line1\nline2""#);
}

// ════════════════════════════════════════════════════════════════════════════
//  §3.2: IRI validation
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn empty_predicate_rejected() {
    let mut g = RdfGraph::new();
    let result = g.add_triple(alice(), "", RdfTerm::literal("x"));
    assert!(matches!(result, Err(CrdfError::InvalidPredicate)));
}

#[test]
fn empty_iri_term_can_be_constructed() {
    // Note: RdfTerm::iri does not currently validate IRI syntax.
    // This test documents the current behavior.
    let term = RdfTerm::iri("");
    assert!(term.is_iri());
    assert_eq!(term.as_iri(), Some(""));
}

#[test]
fn whitespace_iri_term_can_be_constructed() {
    // Documents that IRI syntax validation is not performed
    let term = RdfTerm::iri("not a valid iri");
    assert!(term.is_iri());
}

// ════════════════════════════════════════════════════════════════════════════
//  §3.1: Triple subject validation — Triple::new() vs RdfGraph::add_triple()
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn triple_new_does_not_validate_subject() {
    // Triple::new() is a plain data constructor with no validation.
    // Only RdfGraph::add_triple() enforces subject constraints.
    let lit_subject = RdfTerm::Literal(Literal::new("not a valid subject"));
    let triple = Triple::new(lit_subject.clone(), FOAF_NAME, RdfTerm::literal("x"));
    assert_eq!(triple.subject, lit_subject);
}

#[test]
fn add_triple_rejects_literal_subject() {
    let mut g = RdfGraph::new();
    let lit = RdfTerm::Literal(Literal::new("not a subject"));
    let result = g.add_triple(lit, FOAF_NAME, RdfTerm::literal("x"));
    assert!(matches!(result, Err(CrdfError::LiteralSubject)));
}

#[test]
fn add_triple_accepts_iri_subject() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    assert_eq!(g.len(), 1);
}

#[test]
fn add_triple_accepts_blank_node_subject() {
    let mut g = RdfGraph::new();
    g.add_triple(
        RdfTerm::blank_node("b0"),
        FOAF_NAME,
        RdfTerm::literal("Anon"),
    )
    .unwrap();
    assert_eq!(g.len(), 1);
}

// ════════════════════════════════════════════════════════════════════════════
//  §3.3: Literal equality — same datatype required for equality
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn simple_literal_has_xsd_string_datatype() {
    let lit = Literal::new("hello");
    assert_eq!(lit.datatype(), XSD_STRING);
}

#[test]
fn from_str_literal_equals_new_literal() {
    // Both should produce xsd:string literals
    let a = Literal::new("hello");
    let b = Literal::from("hello");
    assert_eq!(a, b);
}

#[test]
fn typed_literal_not_equal_to_simple_literal() {
    let simple = Literal::new("42");
    let typed = Literal::new("42").with_datatype(XSD_INTEGER).unwrap();
    assert_ne!(simple, typed);
}

#[test]
fn language_tagged_literal_not_equal_to_simple_literal() {
    let simple = Literal::new("hello");
    let tagged = Literal::new("hello").with_language("en").unwrap();
    assert_ne!(simple, tagged);
}

// ════════════════════════════════════════════════════════════════════════════
//  §3.1: Predicate is an IRI — only IRI predicates allowed
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn valid_iri_predicate_accepted() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), "http://example.org/pred", RdfTerm::literal("v"))
        .unwrap();
    assert_eq!(g.len(), 1);
}

#[test]
fn empty_string_predicate_rejected() {
    let mut g = RdfGraph::new();
    let result = g.add_triple(alice(), "", RdfTerm::literal("v"));
    assert!(matches!(result, Err(CrdfError::InvalidPredicate)));
}

// ════════════════════════════════════════════════════════════════════════════
//  §3.3: Display format consistency with RDF term equality
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn equal_literals_have_equal_display() {
    let a = Literal::new("hello").with_language("en").unwrap();
    let b = Literal::new("hello").with_language("EN").unwrap(); // normalized to "en"
    assert_eq!(a, b);
    assert_eq!(a.to_string(), b.to_string());
}

#[test]
fn different_datatype_literals_have_different_display() {
    let a = Literal::new("42"); // xsd:string
    let b = Literal::new("42").with_datatype(XSD_INTEGER).unwrap();
    assert_ne!(a.to_string(), b.to_string());
}
