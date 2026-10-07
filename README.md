# crdf

[![License](https://img.shields.io/crates/l/crdf?style=flat-square)](https://crates.io/crates/crdf)
[![GitHub](https://img.shields.io/badge/GitHub-bkbkb--net%2Fcrdf-181717?style=flat-square&logo=github)](https://github.com/bkbkb-net/crdf)

A **CRDT-based RDF graph** workspace in Rust, built on top of [crdt-graph](https://github.com/bkbkb-net/crdt-graph).

## Workspace

This repository is a Cargo workspace containing the following crates:

| Crate | Description | Links |
|-------|-------------|-------|
| [**crdf**](crdf/) | Core library — CRDT-based RDF graph with replication, pattern matching, and file I/O | [![Crates.io](https://img.shields.io/crates/v/crdf?style=flat-square)](https://crates.io/crates/crdf) [![Docs.rs](https://img.shields.io/docsrs/crdf?style=flat-square)](https://docs.rs/crdf) |
| [**crdf-circuit**](crdf-circuit/) | Synchronous NAND+register circuits persisted as CRDF, with bit-sliced evaluation and optional JIT | — |
| [**crdf-dpo**](crdf-dpo/) | Double Pushout (DPO) graph rewriting for RDF graphs | — |
| [**crdf-editor**](crdf-editor/) | Visual RDF graph editor powered by egui (with DPO support) | — |

## Overview

`crdf` provides an RDF graph that can be replicated across multiple nodes using operation-based CRDTs. Each RDF term (IRI, blank node, or literal) maps to a vertex, and each triple becomes a directed edge in the underlying 2P2P-Graph CRDT.

Key properties:

- **Conflict-free** — Concurrent additions on different replicas converge automatically.
- **Op-based replication** — Operations returned by `add_triple` / `remove_triple` can be broadcast and applied on remote replicas via `apply_downstream`.
- **RDF 1.1 compliant** — Literals always carry a datatype IRI; language-tagged literals use `rdf:langString`; language tags are normalized to lowercase.
- **FlatBuffers serialization** — Save and load graphs in a compact `.crdf` binary format.

## Quick Start

```rust
use crdf::{RdfGraph, RdfTerm};

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
```

## Building

The development workspace builds against a `crdt-graph` checkout beside the
`crdf` checkout: `../crdt-graph` relative to this repository's root. From the
`crdf` repository root, get it with:

```sh
git clone https://github.com/bkbkb-net/crdt-graph.git ../crdt-graph
```

CI builds against the revision named by `CRDT_GRAPH_REV` in
`.github/workflows/ci.yml`; check that one out for the same result.

Then build the workspace:

```sh
cargo build --workspace
```

## Testing

```sh
cargo test --workspace
```

## License

Licensed under either of

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

at your option.

Dependency, provenance and preliminary patent review notes are maintained in
[LEGAL_REVIEW.md](LEGAL_REVIEW.md). Run `cargo deny check licenses sources`
before accepting dependency updates or preparing a release.
