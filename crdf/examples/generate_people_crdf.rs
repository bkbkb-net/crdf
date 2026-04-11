/// Generates `tests/fixtures/people.crdf` from the same data as `people.nt`.
///
/// Run with: cargo run --example generate_people_crdf
use crdf::{Literal, RdfFileFormat, RdfGraph, RdfTerm};

fn main() {
    let mut g = RdfGraph::new();

    g.add_triple(
        RdfTerm::iri("http://example.org/alice"),
        "http://xmlns.com/foaf/0.1/name",
        RdfTerm::literal("Alice"),
    )
    .unwrap();

    g.add_triple(
        RdfTerm::iri("http://example.org/alice"),
        "http://xmlns.com/foaf/0.1/knows",
        RdfTerm::iri("http://example.org/bob"),
    )
    .unwrap();

    g.add_triple(
        RdfTerm::iri("http://example.org/bob"),
        "http://xmlns.com/foaf/0.1/name",
        RdfTerm::literal("Bob"),
    )
    .unwrap();

    g.add_triple(
        RdfTerm::iri("http://example.org/bob"),
        "http://xmlns.com/foaf/0.1/mbox",
        RdfTerm::iri("mailto:bob@example.org"),
    )
    .unwrap();

    // Also add a remove so CRDT history is non-trivial (and we can test it)
    g.add_triple(
        RdfTerm::iri("http://example.org/alice"),
        "http://xmlns.com/foaf/0.1/age",
        RdfTerm::Literal(
            Literal::new("30")
                .with_datatype("http://www.w3.org/2001/XMLSchema#integer")
                .unwrap(),
        ),
    )
    .unwrap();
    g.remove_triple(
        &RdfTerm::iri("http://example.org/alice"),
        "http://xmlns.com/foaf/0.1/age",
        &RdfTerm::Literal(
            Literal::new("30")
                .with_datatype("http://www.w3.org/2001/XMLSchema#integer")
                .unwrap(),
        ),
    )
    .unwrap();

    let path = std::path::Path::new("tests/fixtures/people.crdf");
    g.write_rdf_file(path, RdfFileFormat::FlatBuffers).unwrap();

    println!("Generated: {}", path.display());
    println!("  Active triples: {}", g.len());
    println!("  V_A: {}", g.all_vertices_added().len());
    println!("  V_R: {}", g.all_vertices_removed().len());
    println!("  E_A: {}", g.all_edges_added().len());
    println!("  E_R: {}", g.all_edges_removed().len());
}
