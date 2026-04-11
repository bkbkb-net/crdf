# crdf

[![Crates.io](https://img.shields.io/crates/v/crdf?style=flat-square)](https://crates.io/crates/crdf)
[![Docs.rs](https://img.shields.io/docsrs/crdf?style=flat-square)](https://docs.rs/crdf)
[![License](https://img.shields.io/crates/l/crdf?style=flat-square)](https://crates.io/crates/crdf)
[![GitHub](https://img.shields.io/badge/GitHub-bkbkb--net%2Fcrdf-181717?style=flat-square&logo=github)](https://github.com/bkbkb-net/crdf)


A **CRDT-based RDF graph** implementation in Rust, built on top of [crdt-graph](https://github.com/bkbkb-net/crdt-graph).

## Overview

`crdf` provides an RDF graph that can be replicated across multiple nodes using operation-based CRDTs. Each RDF term (IRI, blank node, or literal) maps to a vertex, and each triple becomes a directed edge in the underlying 2P2P-Graph CRDT.

Key properties:

- **Conflict-free** — Concurrent additions on different replicas converge automatically.
- **Op-based replication** — Operations returned by `add_triple` / `remove_triple` can be broadcast and applied on remote replicas via `apply_downstream`.
- **RDF 1.1 compliant** — Literals always carry a datatype IRI; language-tagged literals use `rdf:langString`; language tags are normalized to lowercase.

## Features

- **RDF term types** — `RdfTerm::Iri`, `RdfTerm::BlankNode`, `RdfTerm::Literal` with full accessor API.
- **Typed literals** — `From` conversions for `bool`, `i32`, `i64`, `f32`, `f64`, `&str`, and `String`.
- **XSD constants** — `XSD_STRING`, `XSD_INTEGER`, `XSD_INT`, `XSD_DOUBLE`, `XSD_FLOAT`, `XSD_BOOLEAN`.
- **Pattern matching** — `triples_matching(subject?, predicate?, object?)` with wildcard support.
- **Graph queries** — `subjects()`, `predicates()`, `objects()`, `triples_for_subject()`, and more.
- **N-Triples display** — `Display` implementations output valid N-Triples syntax.
- **oxrdf conversion** — Convert terms, triples, and whole graphs into `oxrdf` types.
- **RDF file output** — Export a graph to an RDF file (currently N-Triples).
- **Error handling** — All fallible operations return `Result<_, CrdfError>` via `thiserror`.
- **UUID v7 identifiers** — Time-ordered, globally unique IDs for vertices and edges.

## Quick Start

```rust
use crdf::{RdfGraph, RdfTerm, Literal};

# fn main() {
let mut replica_a = RdfGraph::new();
let mut replica_b = RdfGraph::new();

let alice = RdfTerm::iri("http://example.org/alice");
let bob = RdfTerm::iri("http://example.org/bob");

// Replica A: add triples
let op1 = replica_a
    .add_triple(alice.clone(), "http://xmlns.com/foaf/0.1/name", RdfTerm::literal("Alice"))
    .unwrap();
let op2 = replica_a
    .add_triple(alice.clone(), "http://xmlns.com/foaf/0.1/knows", bob.clone())
    .unwrap();

// Broadcast to Replica B
replica_b.apply_downstream(op1).unwrap();
replica_b.apply_downstream(op2).unwrap();

// Both replicas have converged
assert_eq!(replica_a.len(), 2);
assert_eq!(replica_b.len(), 2);

// Query
let names = replica_b.objects_for_subject_predicate(
    &alice,
    "http://xmlns.com/foaf/0.1/name",
);
assert_eq!(names.len(), 1);
# }
```

### Typed Literals

```rust
use crdf::Literal;

# fn main() {
let integer_lit = Literal::new("42").with_datatype("http://www.w3.org/2001/XMLSchema#integer").unwrap();
let lang_lit = Literal::new("hello").with_language("en").unwrap();
let bool_lit = Literal::from(true);
let float_lit = Literal::from(3.14f64);
# }
```

### Export to RDF File

```rust
use crdf::{RdfFileFormat, RdfGraph, RdfTerm};

# fn main() {
let mut graph = RdfGraph::new();
graph
    .add_triple(
        RdfTerm::iri("http://example.org/alice"),
        "http://xmlns.com/foaf/0.1/name",
        RdfTerm::literal("Alice"),
    )
    .unwrap();

// Convert to oxrdf internally and write as N-Triples RDF file
graph
    .write_rdf_file("people.nt", RdfFileFormat::NTriples)
    .unwrap();
# }
```

## API

| Method | Description |
|--------|-------------|
| `RdfGraph::new()` | Creates an empty RDF graph. |
| `add_triple(s, p, o)` | Adds a triple (atSource). Returns the operation to broadcast. |
| `remove_triple(s, p, o)` | Removes a triple (atSource). Returns the operation to broadcast. |
| `apply_downstream(op)` | Applies an operation received from a remote replica. |
| `contains_triple(s, p, o)` | Returns `true` if the triple exists. |
| `triples()` | Returns all active triples. |
| `triples_matching(s?, p?, o?)` | Pattern-based triple lookup. |
| `subjects()` / `predicates()` / `objects()` | Unique terms in each position. |
| `to_oxrdf()` | Converts the graph to `oxrdf::Graph`. |
| `write_rdf_file(path, format)` | Writes an RDF file from the graph. |
| `len()` / `is_empty()` | Triple count and emptiness check. |

## Errors

All fallible operations return `Result<_, CrdfError>`:

| Variant | Description |
|---------|-------------|
| `Graph(TwoPTwoPGraphError)` | Error from the underlying CRDT graph. |
| `TripleNotFound` | The specified triple was not found. |
| `LiteralSubject` | Literals cannot be used as subjects. |
| `InvalidPredicate` | Predicates must be non-empty IRIs. |
| `EmptyLanguageTag` | Language tags must be non-empty (BCP 47). |
| `LangStringDatatype` | `rdf:langString` cannot be set directly; use `with_language()`. |
