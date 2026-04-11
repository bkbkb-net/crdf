use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;

use crdf_editor::app::CrdfEditorApp;

fn create_test_harness<'a>() -> Harness<'a, CrdfEditorApp> {
    Harness::builder()
        .with_size(egui::Vec2::new(1280.0, 800.0))
        .build_eframe(|_cc| CrdfEditorApp::new())
}

#[test]
fn test_initial_state_shows_zero_triples() {
    let harness = create_test_harness();
    // The app should show "Triples: 0  |  Nodes: 0"
    assert!(harness.query_by_label_contains("Triples: 0").is_some());
}

#[test]
fn test_menu_bar_exists() {
    let harness = create_test_harness();
    assert!(harness.query_by_label("File").is_some());
    assert!(harness.query_by_label("Edit").is_some());
    assert!(harness.query_by_label("View").is_some());
}

#[test]
fn test_undo_button_disabled_initially() {
    let mut harness = create_test_harness();
    // Open Edit menu
    harness.get_by_label("Edit").click();
    harness.run();
    let undo_btn = harness.get_by_label_contains("Undo");
    // Button should exist but be disabled (no actions to undo)
    assert!(undo_btn.accesskit_node().is_disabled());
}

#[test]
fn test_redo_button_disabled_initially() {
    let mut harness = create_test_harness();
    // Open Edit menu
    harness.get_by_label("Edit").click();
    harness.run();
    let redo_btn = harness.get_by_label_contains("Redo");
    assert!(redo_btn.accesskit_node().is_disabled());
}

#[test]
fn test_add_triple_button_exists() {
    let harness = create_test_harness();
    // The "Add Triple" collapsing header should be present
    assert!(harness.query_by_label_contains("Add Triple").is_some());
}

#[test]
fn test_crdt_state_panel_exists() {
    let harness = create_test_harness();
    assert!(harness.query_by_label_contains("CRDT State").is_some());
}

#[test]
fn test_zoom_display_shows_100_percent() {
    let harness = create_test_harness();
    assert!(harness.query_by_label_contains("Zoom: 100%").is_some());
}

#[test]
fn test_heading_exists() {
    let harness = create_test_harness();
    assert!(harness.query_by_label_contains("crdf Editor").is_some());
}

#[test]
fn test_add_triple_then_undo_enabled() {
    let mut harness = create_test_harness();

    // Add a triple via state mutation
    {
        let app = harness.state_mut();
        let subject = crdf::RdfTerm::iri("http://example.org/Alice");
        let object = crdf::RdfTerm::iri("http://example.org/Bob");
        app.add_triple(subject, "http://xmlns.com/foaf/0.1/knows", object)
            .unwrap();
        app.ensure_node_positions();
    }

    // Re-render
    harness.run();

    // Open Edit menu and check undo is enabled
    harness.get_by_label("Edit").click();
    harness.run();
    let undo_btn = harness.get_by_label_contains("Undo");
    assert!(!undo_btn.accesskit_node().is_disabled());

    // Triple count should be 1
    assert!(harness.query_by_label_contains("Triples: 1").is_some());
}

#[test]
fn test_undo_then_redo_enabled() {
    let mut harness = create_test_harness();

    // Add a triple
    {
        let app = harness.state_mut();
        let subject = crdf::RdfTerm::iri("http://example.org/Alice");
        let object = crdf::RdfTerm::iri("http://example.org/Bob");
        app.add_triple(subject, "http://xmlns.com/foaf/0.1/knows", object)
            .unwrap();
        app.ensure_node_positions();
    }
    harness.run();

    // Undo
    {
        let app = harness.state_mut();
        assert!(app.undo());
        app.remove_orphan_positions();
    }
    harness.run();

    // Open Edit menu and check button states
    harness.get_by_label("Edit").click();
    harness.run();

    // Redo button should be enabled, undo disabled
    let redo_btn = harness.get_by_label_contains("Redo");
    assert!(!redo_btn.accesskit_node().is_disabled());

    let undo_btn = harness.get_by_label_contains("Undo");
    assert!(undo_btn.accesskit_node().is_disabled());

    // Triple count should be back to 0
    assert!(harness.query_by_label_contains("Triples: 0").is_some());
}

#[test]
fn test_multiple_triples_node_count() {
    let mut harness = create_test_harness();

    {
        let app = harness.state_mut();
        let alice = crdf::RdfTerm::iri("http://example.org/Alice");
        let bob = crdf::RdfTerm::iri("http://example.org/Bob");
        let carol = crdf::RdfTerm::iri("http://example.org/Carol");
        app.add_triple(alice.clone(), "http://xmlns.com/foaf/0.1/knows", bob)
            .unwrap();
        app.add_triple(alice, "http://xmlns.com/foaf/0.1/knows", carol)
            .unwrap();
        app.ensure_node_positions();
    }
    harness.run();

    assert!(harness.query_by_label_contains("Triples: 2").is_some());
    assert!(harness.query_by_label_contains("Nodes: 3").is_some());
}

#[test]
fn test_crdt_sets_show_counts() {
    let mut harness = create_test_harness();

    {
        let app = harness.state_mut();
        let alice = crdf::RdfTerm::iri("http://example.org/Alice");
        let bob = crdf::RdfTerm::iri("http://example.org/Bob");
        app.add_triple(alice, "http://xmlns.com/foaf/0.1/knows", bob)
            .unwrap();
        app.ensure_node_positions();
    }
    harness.run();

    // V_A should have 2 vertices, E_A should have 1 edge
    assert!(harness.query_by_label_contains("V_A: 2").is_some());
    assert!(harness.query_by_label_contains("E_A: 1").is_some());
}

#[test]
fn test_undo_redo_cycle_preserves_state() {
    let mut harness = create_test_harness();

    // Add triple
    {
        let app = harness.state_mut();
        let subject = crdf::RdfTerm::iri("http://example.org/Alice");
        let object = crdf::RdfTerm::iri("http://example.org/Bob");
        app.add_triple(subject, "http://xmlns.com/foaf/0.1/knows", object)
            .unwrap();
        app.ensure_node_positions();
    }
    harness.run();
    assert!(harness.query_by_label_contains("Triples: 1").is_some());

    // Undo
    {
        let app = harness.state_mut();
        app.undo();
        app.remove_orphan_positions();
    }
    harness.run();
    assert!(harness.query_by_label_contains("Triples: 0").is_some());

    // Redo
    {
        let app = harness.state_mut();
        app.redo();
        app.ensure_node_positions();
        app.remove_orphan_positions();
    }
    harness.run();
    assert!(harness.query_by_label_contains("Triples: 1").is_some());
}
