//! Editing-layer tests: circuits built and modified purely through
//! [`CircuitEditor`] triple operations on a live graph, then loaded,
//! compiled and executed.

use crdf::RdfGraph;
use crdf_circuit::compile::{CompileError, CompiledCircuit};
use crdf_circuit::editor::CircuitEditor;
use crdf_circuit::eval::CircuitState;
use crdf_circuit::model::{CellKind, Endpoint, PortDirection};
use crdf_circuit::{asset_from_rdf, asset_from_rdf_by_id, list_assets, write_asset_into};

/// Drives a compiled single-input/single-output circuit for one step.
fn eval_bit(circuit: &CompiledCircuit, input_name: &str, output_name: &str, value: bool) -> bool {
    let input = circuit.input_index(input_name).unwrap();
    let output = circuit.output_index(output_name).unwrap();
    let mut state = CircuitState::new(circuit);
    state.set_level(circuit, input, 0, value);
    state.process_step(circuit);
    state.output_bit(output, 0)
}

#[test]
fn a_circuit_built_entirely_through_the_editor_compiles_and_runs() {
    let mut graph = RdfGraph::new();
    let mut editor = CircuitEditor::new(&mut graph);

    // x → NOT → y, built triple-by-triple.
    let module = editor.create_module("edited.not").unwrap();
    let x = editor.add_port(module, "x", PortDirection::Input).unwrap();
    let y = editor.add_port(module, "y", PortDirection::Output).unwrap();
    let gate = editor.add_cell(module, CellKind::Nand).unwrap();
    editor
        .add_wire(module, Endpoint::module(x), Endpoint::nand_a(gate))
        .unwrap();
    editor
        .add_wire(module, Endpoint::module(x), Endpoint::nand_b(gate))
        .unwrap();
    let out_wire = editor
        .add_wire(module, Endpoint::nand_y(gate), Endpoint::module(y))
        .unwrap();
    editor.create_asset(module, 1).unwrap();

    let asset = asset_from_rdf(&graph).unwrap();
    let circuit = CompiledCircuit::compile(&asset).unwrap();
    assert!(eval_bit(&circuit, "x", "y", false));
    assert!(!eval_bit(&circuit, "x", "y", true));

    // Rewire: y now reads x directly (remove old wire + add new one —
    // the only way the API allows changing a connection). The NAND
    // becomes dangling-but-unconsumed and is removed too.
    let mut editor = CircuitEditor::new(&mut graph);
    editor.remove_wire(out_wire).unwrap();
    editor.remove_cell(gate).unwrap();
    editor
        .add_wire(module, Endpoint::module(x), Endpoint::module(y))
        .unwrap();

    let asset = asset_from_rdf(&graph).unwrap();
    let circuit = CompiledCircuit::compile(&asset).unwrap();
    assert!(!eval_bit(&circuit, "x", "y", false));
    assert!(eval_bit(&circuit, "x", "y", true));
}

#[test]
fn removing_a_cell_cascades_its_wires() {
    let mut graph = RdfGraph::new();
    let mut editor = CircuitEditor::new(&mut graph);

    let module = editor.create_module("edited.cascade").unwrap();
    let x = editor.add_port(module, "x", PortDirection::Input).unwrap();
    let y = editor.add_port(module, "y", PortDirection::Output).unwrap();
    let gate = editor.add_cell(module, CellKind::Nand).unwrap();
    editor
        .add_wire(module, Endpoint::module(x), Endpoint::nand_a(gate))
        .unwrap();
    editor
        .add_wire(module, Endpoint::module(x), Endpoint::nand_b(gate))
        .unwrap();
    editor
        .add_wire(module, Endpoint::nand_y(gate), Endpoint::module(y))
        .unwrap();
    editor.create_asset(module, 1).unwrap();

    // Removing the gate removes its three wires: the asset still loads
    // (no dangling endpoint triples) but no longer compiles, because
    // the output lost its driver.
    let mut editor = CircuitEditor::new(&mut graph);
    editor.remove_cell(gate).unwrap();

    let asset = asset_from_rdf(&graph).unwrap();
    let err = CompiledCircuit::compile(&asset).unwrap_err();
    assert!(matches!(err, CompileError::UndrivenTopOutput { .. }));
}

#[test]
fn two_assets_can_share_one_root_at_different_tick_rates() {
    // The exact case the old root-derived asset identity forbade.
    let mut graph = RdfGraph::new();
    let mut editor = CircuitEditor::new(&mut graph);

    // A one-register toggler: y flips every tick, so the observed
    // per-step period depends on ticks_per_step.
    let module = editor.create_module("edited.toggler").unwrap();
    let y = editor.add_port(module, "y", PortDirection::Output).unwrap();
    let reg = editor
        .add_cell(module, CellKind::Reg { init: false })
        .unwrap();
    let inv = editor.add_cell(module, CellKind::Nand).unwrap();
    editor
        .add_wire(module, Endpoint::reg_q(reg), Endpoint::nand_a(inv))
        .unwrap();
    editor
        .add_wire(module, Endpoint::reg_q(reg), Endpoint::nand_b(inv))
        .unwrap();
    editor
        .add_wire(module, Endpoint::nand_y(inv), Endpoint::reg_d(reg))
        .unwrap();
    editor
        .add_wire(module, Endpoint::reg_q(reg), Endpoint::module(y))
        .unwrap();

    let slow = editor.create_asset(module, 1).unwrap();
    let fast = editor.create_asset(module, 2).unwrap();

    let listed = list_assets(&graph).unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.contains(&slow) && listed.contains(&fast));

    let slow_asset = asset_from_rdf_by_id(&graph, slow).unwrap();
    let fast_asset = asset_from_rdf_by_id(&graph, fast).unwrap();
    assert_eq!(slow_asset.root, fast_asset.root);
    assert_eq!(slow_asset.ticks_per_step, 1);
    assert_eq!(fast_asset.ticks_per_step, 2);

    // Outputs are the final tick's pre-latch snapshot. T=1 observes
    // ticks 0,1,2,3… (alternating); T=2 always observes an odd tick,
    // where the toggler reads 1.
    let slow_circuit = CompiledCircuit::compile(&slow_asset).unwrap();
    let fast_circuit = CompiledCircuit::compile(&fast_asset).unwrap();
    let output = slow_circuit.output_index("y").unwrap();
    let mut slow_state = CircuitState::new(&slow_circuit);
    let mut fast_state = CircuitState::new(&fast_circuit);
    let mut slow_seen = Vec::new();
    let mut fast_seen = Vec::new();
    for _ in 0..4 {
        slow_state.process_step(&slow_circuit);
        fast_state.process_step(&fast_circuit);
        slow_seen.push(slow_state.output_bit(output, 0));
        fast_seen.push(fast_state.output_bit(fast_circuit.output_index("y").unwrap(), 0));
    }
    assert_eq!(slow_seen, vec![false, true, false, true]);
    assert_eq!(fast_seen, vec![true, true, true, true]);

    // Removing one asset leaves the other (and the shared module).
    let mut editor = CircuitEditor::new(&mut graph);
    editor.remove_asset(slow).unwrap();
    let listed = list_assets(&graph).unwrap();
    assert_eq!(listed, vec![fast].into_iter().collect::<Vec<_>>());
    assert!(asset_from_rdf_by_id(&graph, fast).is_ok());
}

#[test]
fn bulk_import_then_edit_in_place() {
    // An in-memory asset (e.g. a stdlib circuit) written into a live
    // graph keeps its identity and stays editable there.
    let mut builder = crdf_circuit::model::ModuleBuilder::new("imported");
    let x = builder.input("x");
    let y = builder.output("y");
    builder.wire(Endpoint::module(x), Endpoint::module(y));
    let root = builder.id();
    let mut library = crdf_circuit::model::ModuleLibrary::new();
    library.insert(builder.finish());
    let asset = crdf_circuit::model::CircuitAsset::new(root, 1, library);

    let mut graph = RdfGraph::new();
    write_asset_into(&mut graph, &asset).unwrap();

    let loaded = asset_from_rdf_by_id(&graph, asset.id).unwrap();
    assert_eq!(loaded.id, asset.id);
    let circuit = CompiledCircuit::compile(&loaded).unwrap();
    assert!(eval_bit(&circuit, "x", "y", true));

    // Continue editing the imported module in the live graph: add an
    // inverted output driven by a NAND of the pass-through.
    let mut editor = CircuitEditor::new(&mut graph);
    let z = editor.add_port(root, "z", PortDirection::Output).unwrap();
    let gate = editor.add_cell(root, CellKind::Nand).unwrap();
    editor
        .add_wire(root, Endpoint::module(x), Endpoint::nand_a(gate))
        .unwrap();
    editor
        .add_wire(root, Endpoint::module(x), Endpoint::nand_b(gate))
        .unwrap();
    editor
        .add_wire(root, Endpoint::nand_y(gate), Endpoint::module(z))
        .unwrap();

    let loaded = asset_from_rdf_by_id(&graph, asset.id).unwrap();
    let circuit = CompiledCircuit::compile(&loaded).unwrap();
    assert!(eval_bit(&circuit, "x", "y", true));
    assert!(!eval_bit(&circuit, "x", "z", true));
}

#[test]
fn basic_properties_can_change_without_reidentifying_resources() {
    let mut graph = RdfGraph::new();
    let mut editor = CircuitEditor::new(&mut graph);
    let module = editor.create_module("properties").unwrap();
    let port = editor
        .add_port(module, "before", PortDirection::Input)
        .unwrap();
    let cell = editor
        .add_cell(module, CellKind::Reg { init: true })
        .unwrap();
    let asset = editor.create_asset(module, 1).unwrap();

    editor.set_port_name(port, "after").unwrap();
    editor
        .set_port_direction(port, PortDirection::Output)
        .unwrap();
    editor.set_cell_kind(cell, CellKind::Nand).unwrap();

    let loaded = asset_from_rdf_by_id(&graph, asset).unwrap();
    let root = loaded.library.get(module).unwrap();
    assert_eq!(root.ports[0].id, port);
    assert_eq!(root.ports[0].name, "after");
    assert_eq!(root.ports[0].direction, PortDirection::Output);
    assert_eq!(root.cells[0].id, cell);
    assert_eq!(root.cells[0].kind, CellKind::Nand);
}

#[test]
fn editor_operations_replay_on_a_replica_in_order() {
    let mut source = RdfGraph::new();
    let mut editor = CircuitEditor::new(&mut source);
    let module = editor.create_module("replicated.before").unwrap();
    let input = editor.add_port(module, "x", PortDirection::Input).unwrap();
    let output = editor.add_port(module, "y", PortDirection::Output).unwrap();
    editor
        .add_wire(module, Endpoint::module(input), Endpoint::module(output))
        .unwrap();
    let asset = editor.create_asset(module, 1).unwrap();

    assert!(!editor.operations().is_empty());
    let initial = editor.take_operations();
    assert!(editor.operations().is_empty());

    let mut replica = RdfGraph::new();
    for operation in initial {
        replica.apply_downstream(operation).unwrap();
    }
    assert_eq!(
        asset_from_rdf_by_id(&replica, asset)
            .unwrap()
            .library
            .get(module)
            .unwrap()
            .name,
        "replicated.before"
    );

    editor.set_module_name(module, "replicated.after").unwrap();
    let replacement = editor.into_operations();
    assert_eq!(replacement.len(), 2, "replace is one remove plus one add");
    for operation in replacement {
        replica.apply_downstream(operation).unwrap();
    }

    let source_asset = asset_from_rdf_by_id(&source, asset).unwrap();
    let replica_asset = asset_from_rdf_by_id(&replica, asset).unwrap();
    assert_eq!(replica_asset, source_asset);
    assert_eq!(
        replica_asset.library.get(module).unwrap().name,
        "replicated.after"
    );
}
