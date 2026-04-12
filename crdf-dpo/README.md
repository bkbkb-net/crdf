# crdf-dpo

Double Pushout (DPO) graph rewriting for CRDF RDF graphs.

## Overview

This crate implements the **Double Pushout (DPO)** algebraic graph transformation approach on top of [`crdf`](https://crates.io/crates/crdf) RDF graphs. A DPO rule is defined as a span **L ← K → R**:

- **L** (left-hand side): A pattern of triples to match in the host graph
- **K** (interface): The subset of triples preserved during transformation (must be a subpattern of both L and R)
- **R** (right-hand side): The replacement pattern

When a rule is applied:
1. A match of L is found in the host graph G
2. Triples matched by L but not in K are **deleted** from G
3. Triples in R but not in K are **added**, producing the result graph H

The **dangling condition** and **identification condition** are checked to ensure safe rewriting.

## Example

```rust
use crdf::{RdfGraph, RdfTerm};
use crdf_dpo::{DpoRule, PatternTerm, PatternPredicate, PatternTriple};

let mut graph = RdfGraph::new();
graph.add_triple(
    RdfTerm::iri("http://example.org/Alice"),
    "http://example.org/livesIn",
    RdfTerm::iri("http://example.org/Tokyo"),
).unwrap();
graph.add_triple(
    RdfTerm::iri("http://example.org/Alice"),
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
    RdfTerm::iri("http://example.org/Person"),
).unwrap();

let person_type = PatternTriple::new(
    PatternTerm::concrete(RdfTerm::iri("http://example.org/Alice")),
    PatternPredicate::concrete("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
    PatternTerm::concrete(RdfTerm::iri("http://example.org/Person")),
);

let lives_in_tokyo = PatternTriple::new(
    PatternTerm::concrete(RdfTerm::iri("http://example.org/Alice")),
    PatternPredicate::concrete("http://example.org/livesIn"),
    PatternTerm::concrete(RdfTerm::iri("http://example.org/Tokyo")),
);

// Rule: Relocate Alice from Tokyo to Osaka
// L: Alice livesIn Tokyo, Alice a Person
// K: Alice a Person (preserved)
// R: Alice livesIn Osaka, Alice a Person
let rule = DpoRule::new(
    "relocate",
    vec![lives_in_tokyo, person_type.clone()],  // L
    vec![person_type.clone()],                   // K
    vec![
        PatternTriple::new(
            PatternTerm::concrete(RdfTerm::iri("http://example.org/Alice")),
            PatternPredicate::concrete("http://example.org/livesIn"),
            PatternTerm::concrete(RdfTerm::iri("http://example.org/Osaka")),
        ),
        person_type,                              // R
    ],
);

rule.apply(&mut graph).unwrap();
assert!(graph.contains_triple(
    &RdfTerm::iri("http://example.org/Alice"),
    "http://example.org/livesIn",
    &RdfTerm::iri("http://example.org/Osaka"),
));
```

## Builder API

The `DpoRuleBuilder` provides a fluent interface for constructing rules. Shorthand helpers `iri()`, `literal()`, and `pred()` simplify pattern construction:

```rust
use crdf_dpo::{DpoRuleBuilder, iri, literal, pred};

let rule = DpoRuleBuilder::new("birthday")
    .lhs_triple(
        iri("http://example.org/Alice"),
        pred("http://example.org/age"),
        literal("30"),
    )
    .lhs_triple(
        iri("http://example.org/Alice"),
        pred("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
        iri("http://example.org/Person"),
    )
    .interface_triple(
        iri("http://example.org/Alice"),
        pred("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
        iri("http://example.org/Person"),
    )
    .rhs_triple(
        iri("http://example.org/Alice"),
        pred("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
        iri("http://example.org/Person"),
    )
    .rhs_triple(
        iri("http://example.org/Alice"),
        pred("http://example.org/age"),
        literal("31"),
    )
    .build()
    .unwrap();
```

## DPO Rules as RDF

Rules can be serialized to and deserialized from RDF using the `dpo:` vocabulary (`http://crdf.bkbkb.net/dpo#`):

```rust
use crdf::RdfGraph;
use crdf_dpo::DpoRule;

# let rule = crdf_dpo::DpoRule::new("example", vec![], vec![], vec![]);
// Save
let mut store = RdfGraph::new();
rule.to_rdf(&mut store, "http://example.org/rules/my_rule").unwrap();

// Load
let loaded = DpoRule::from_rdf(&store, "http://example.org/rules/my_rule").unwrap();
```

### DPO Vocabulary

| URI | Type | Description |
|-----|------|-------------|
| `dpo:Rule` | Class | A DPO rewriting rule |
| `dpo:Pattern` | Class | A graph pattern (L, K, or R) |
| `dpo:PatternTriple` | Class | A triple within a pattern |
| `dpo:name` | Property | Name of a rule |
| `dpo:lhs` | Property | Links rule → LHS pattern |
| `dpo:interface` | Property | Links rule → interface pattern |
| `dpo:rhs` | Property | Links rule → RHS pattern |
| `dpo:triple` | Property | Links pattern → constituent triple |
| `dpo:subject` | Property | Subject of a pattern triple |
| `dpo:predicate` | Property | Predicate of a pattern triple |
| `dpo:object` | Property | Object of a pattern triple |

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
