use crdf::{RdfGraph, RdfTerm};
use crdf_circuit::editor::CircuitEditor;
use crdf_circuit::model::{
    CellKind, CircuitAsset, Endpoint, ModuleBuilder, ModuleId, ModuleLibrary,
};
use crdf_circuit::vocab;
use crdf_circuit::{
    CircuitCrdfError, asset_from_rdf_by_id, write_asset_into, write_asset_into_with_operations,
};

fn passthrough_asset() -> CircuitAsset {
    let mut module = ModuleBuilder::new("passthrough");
    let input = module.input("x");
    let output = module.output("y");
    module.wire(Endpoint::module(input), Endpoint::module(output));
    let root = module.id();
    let mut library = ModuleLibrary::new();
    library.insert(module.finish());
    CircuitAsset::new(root, 1, library)
}

#[test]
fn importing_the_same_asset_twice_is_idempotent() {
    let asset = passthrough_asset();
    let mut graph = RdfGraph::new();

    write_asset_into(&mut graph, &asset).unwrap();
    let triples = graph.len();
    let added_edges = graph.all_edges_added().len();
    let added_vertices = graph.all_vertices_added().len();

    write_asset_into(&mut graph, &asset).unwrap();
    assert_eq!(graph.len(), triples);
    assert_eq!(graph.all_edges_added().len(), added_edges);
    assert_eq!(graph.all_vertices_added().len(), added_vertices);
}

#[test]
fn bulk_import_operations_replay_on_a_replica() {
    let asset = passthrough_asset();
    let mut source = RdfGraph::new();
    let operations = write_asset_into_with_operations(&mut source, &asset).unwrap();
    assert!(!operations.is_empty());

    let mut replica = RdfGraph::new();
    for operation in operations {
        replica.apply_downstream(operation).unwrap();
    }
    assert_eq!(
        asset_from_rdf_by_id(&replica, asset.id).unwrap(),
        asset_from_rdf_by_id(&source, asset.id).unwrap()
    );

    assert!(
        write_asset_into_with_operations(&mut source, &asset)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn failed_import_leaves_the_destination_unchanged() {
    let valid = passthrough_asset();
    let mut graph = RdfGraph::new();
    write_asset_into(&mut graph, &valid).unwrap();
    let before_triples = graph.triples();
    let before_added_edges = graph.all_edges_added().len();

    let invalid = CircuitAsset::new(ModuleId::new_random(), 1, ModuleLibrary::new());
    assert!(matches!(
        write_asset_into(&mut graph, &invalid),
        Err(CircuitCrdfError::DanglingReference { .. })
    ));
    assert_eq!(graph.triples(), before_triples);
    assert_eq!(graph.all_edges_added().len(), before_added_edges);
}

#[test]
fn conflicting_reimport_is_rejected_atomically() {
    let asset = passthrough_asset();
    let mut graph = RdfGraph::new();
    write_asset_into(&mut graph, &asset).unwrap();
    let before = graph.triples();

    let mut changed = asset.clone();
    changed.ticks_per_step = 2;
    assert!(matches!(
        write_asset_into(&mut graph, &changed),
        Err(CircuitCrdfError::ConflictingValues { .. })
    ));
    assert_eq!(graph.triples(), before);
    assert_eq!(
        asset_from_rdf_by_id(&graph, asset.id)
            .unwrap()
            .ticks_per_step,
        1
    );
}

#[test]
fn malformed_unrelated_module_does_not_poison_an_asset() {
    let asset = passthrough_asset();
    let mut graph = RdfGraph::new();
    write_asset_into(&mut graph, &asset).unwrap();

    // This module is deliberately missing circuit:moduleName, but no
    // selected asset reaches it.
    let unrelated = RdfTerm::iri(format!("{}{}", vocab::MODULE_PREFIX, uuid::Uuid::new_v4()));
    graph
        .add_triple(unrelated, vocab::RDF_TYPE, RdfTerm::iri(vocab::TYPE_MODULE))
        .unwrap();

    let loaded = asset_from_rdf_by_id(&graph, asset.id).unwrap();
    assert_eq!(loaded.root, asset.root);
    assert_eq!(loaded.library.len(), 1);
}

#[test]
fn malformed_reachable_module_still_fails_the_load() {
    let asset = passthrough_asset();
    let mut graph = RdfGraph::new();
    write_asset_into(&mut graph, &asset).unwrap();

    let missing = ModuleId::new_random();
    CircuitEditor::new(&mut graph)
        .add_cell(asset.root, CellKind::Instance { module: missing })
        .unwrap();

    assert!(matches!(
        asset_from_rdf_by_id(&graph, asset.id),
        Err(CircuitCrdfError::DanglingReference { .. })
    ));
}
