//! Canonical (UUIDv5) identity tests: the standard library must build
//! to byte-identical entities everywhere, so shared modules
//! deduplicate across assets, files and processes.

use crdf::{RdfGraph, RdfTerm};
use crdf_circuit::compile::CompiledCircuit;
use crdf_circuit::editor::CircuitEditor;
use crdf_circuit::model::{CellKind, Endpoint, PortDirection};
use crdf_circuit::stdlib::Stdlib;
use crdf_circuit::vocab;
use crdf_circuit::{asset_from_rdf_by_id, write_asset_into};

#[test]
fn stdlib_builds_are_identical_across_calls() {
    let a = Stdlib::build();
    let b = Stdlib::build();
    assert_eq!(a.library, b.library, "stdlib identities must be canonical");
    assert_eq!(a.full_adder, b.full_adder);
    assert_eq!(a.adder24, b.adder24);
    assert_eq!(a.mul16, b.mul16);
    assert_eq!(a.fmul32, b.fmul32);
    assert_eq!(a.umul24, b.umul24);
    assert_eq!(a.lfsr16, b.lfsr16);
}

#[test]
fn two_assets_share_one_stdlib_copy_in_a_graph() {
    // Two independently authored circuits, each instancing the same
    // canonical stdlib module, written into one graph: the stdlib
    // modules must appear exactly once.
    let stdlib_a = Stdlib::build();
    let stdlib_b = Stdlib::build();

    let make_user_asset = |stdlib: &Stdlib, tag: &str| {
        let mut m = crdf_circuit::model::ModuleBuilder::new(format!("user.{tag}"));
        let x = m.input("x");
        let y = m.input("y");
        let out = m.output("z");
        let gate = m.instance(stdlib.and_gate);
        let a = crdf_circuit::stdlib::port_id(&stdlib.library, stdlib.and_gate, "a");
        let b = crdf_circuit::stdlib::port_id(&stdlib.library, stdlib.and_gate, "b");
        let yy = crdf_circuit::stdlib::port_id(&stdlib.library, stdlib.and_gate, "y");
        m.wire(Endpoint::module(x), Endpoint::inner(gate, a));
        m.wire(Endpoint::module(y), Endpoint::inner(gate, b));
        m.wire(Endpoint::inner(gate, yy), Endpoint::module(out));
        let root = m.id();
        let mut library = stdlib.library.clone();
        library.insert(m.finish());
        crdf_circuit::model::CircuitAsset::new(root, 1, library)
    };

    let asset_a = make_user_asset(&stdlib_a, "a");
    let asset_b = make_user_asset(&stdlib_b, "b");

    let mut graph = RdfGraph::new();
    write_asset_into(&mut graph, &asset_a).unwrap();
    write_asset_into(&mut graph, &asset_b).unwrap();

    // Modules present: 2 user modules + one shared copy of std.and
    // (built from raw NANDs, so its closure is itself). The CRDT may
    // hold duplicate identical triples, so count distinct subjects.
    let modules: std::collections::HashSet<_> = graph
        .subjects_for_predicate_object(vocab::RDF_TYPE, &RdfTerm::iri(vocab::TYPE_MODULE))
        .into_iter()
        .collect();
    assert_eq!(
        modules.len(),
        3,
        "stdlib modules must deduplicate: {modules:?}"
    );

    // Both assets still load and compute AND on their shared module.
    for id in [asset_a.id, asset_b.id] {
        let loaded = asset_from_rdf_by_id(&graph, id).unwrap();
        let circuit = CompiledCircuit::compile(&loaded).unwrap();
        let x = circuit.input_index("x").unwrap();
        let y = circuit.input_index("y").unwrap();
        let z = circuit.output_index("z").unwrap();
        let mut state = crdf_circuit::eval::CircuitState::new(&circuit);
        state.set_level(&circuit, x, 0, true);
        state.set_level(&circuit, y, 0, true);
        state.process_step(&circuit);
        assert!(state.output_bit(z, 0));
    }
}

#[test]
fn user_circuits_keep_random_identities() {
    // Non-canonical builders must not collide: two identically shaped
    // user modules are distinct resources.
    let mut graph = RdfGraph::new();
    let mut editor = CircuitEditor::new(&mut graph);
    let m1 = editor.create_module("same-name").unwrap();
    let m2 = editor.create_module("same-name").unwrap();
    assert_ne!(m1, m2);
    let p1 = editor.add_port(m1, "x", PortDirection::Input).unwrap();
    let p2 = editor.add_port(m2, "x", PortDirection::Input).unwrap();
    assert_ne!(p1, p2);
    let c1 = editor.add_cell(m1, CellKind::Nand).unwrap();
    let c2 = editor.add_cell(m2, CellKind::Nand).unwrap();
    assert_ne!(c1, c2);
}
