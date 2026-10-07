use crdf::{RdfGraph, RdfTerm, TransactionalUndoManager, UndoManager};

// ── Helpers ───────────────────────────────────────────────────

const FOAF_NAME: &str = "http://xmlns.com/foaf/0.1/name";
const FOAF_KNOWS: &str = "http://xmlns.com/foaf/0.1/knows";
const FOAF_AGE: &str = "http://xmlns.com/foaf/0.1/age";

fn alice() -> RdfTerm {
    RdfTerm::iri("http://example.org/alice")
}
fn bob() -> RdfTerm {
    RdfTerm::iri("http://example.org/bob")
}
fn charlie() -> RdfTerm {
    RdfTerm::iri("http://example.org/charlie")
}

fn setup_graph_with_undo() -> (RdfGraph, UndoManager) {
    (RdfGraph::new(), UndoManager::new())
}

// ════════════════════════════════════════════════════════════════
//  Basic Undo
// ════════════════════════════════════════════════════════════════

#[test]
fn undo_add_triple() {
    let (mut g, mut um) = setup_graph_with_undo();
    um.add_triple(&mut g, alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    assert_eq!(g.len(), 1);

    let undo_op = um.undo(&mut g).unwrap();
    assert!(undo_op.is_some());
    assert_eq!(g.len(), 0);
    assert!(!g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
}

#[test]
fn undo_remove_triple() {
    let (mut g, mut um) = setup_graph_with_undo();
    um.add_triple(&mut g, alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    um.remove_triple(&mut g, &alice(), FOAF_NAME, &RdfTerm::literal("Alice"))
        .unwrap();
    assert_eq!(g.len(), 0);

    let undo_op = um.undo(&mut g).unwrap();
    assert!(undo_op.is_some());
    assert_eq!(g.len(), 1);
    assert!(g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
}

#[test]
fn undo_on_empty_stack_returns_none() {
    let (mut g, mut um) = setup_graph_with_undo();
    let result = um.undo(&mut g).unwrap();
    assert!(result.is_none());
}

// ════════════════════════════════════════════════════════════════
//  Basic Redo
// ════════════════════════════════════════════════════════════════

#[test]
fn redo_add_triple() {
    let (mut g, mut um) = setup_graph_with_undo();
    um.add_triple(&mut g, alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    um.undo(&mut g).unwrap();
    assert_eq!(g.len(), 0);

    let redo_op = um.redo(&mut g).unwrap();
    assert!(redo_op.is_some());
    assert_eq!(g.len(), 1);
    assert!(g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
}

#[test]
fn redo_remove_triple() {
    let (mut g, mut um) = setup_graph_with_undo();
    um.add_triple(&mut g, alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    um.remove_triple(&mut g, &alice(), FOAF_NAME, &RdfTerm::literal("Alice"))
        .unwrap();

    um.undo(&mut g).unwrap();
    assert_eq!(g.len(), 1);

    let redo_op = um.redo(&mut g).unwrap();
    assert!(redo_op.is_some());
    assert_eq!(g.len(), 0);
    assert!(!g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
}

#[test]
fn redo_on_empty_stack_returns_none() {
    let (mut g, mut um) = setup_graph_with_undo();
    let result = um.redo(&mut g).unwrap();
    assert!(result.is_none());
}

// ════════════════════════════════════════════════════════════════
//  Undo/Redo sequences
// ════════════════════════════════════════════════════════════════

#[test]
fn undo_redo_roundtrip() {
    let (mut g, mut um) = setup_graph_with_undo();
    um.add_triple(&mut g, alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    assert_eq!(g.len(), 1);

    um.undo(&mut g).unwrap();
    assert_eq!(g.len(), 0);

    um.redo(&mut g).unwrap();
    assert_eq!(g.len(), 1);
    assert!(g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
}

#[test]
fn multiple_undo_redo() {
    let (mut g, mut um) = setup_graph_with_undo();
    um.add_triple(&mut g, alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    um.add_triple(&mut g, alice(), FOAF_KNOWS, bob()).unwrap();
    um.add_triple(&mut g, bob(), FOAF_NAME, RdfTerm::literal("Bob"))
        .unwrap();
    assert_eq!(g.len(), 3);

    um.undo(&mut g).unwrap(); // undo Bob name
    assert_eq!(g.len(), 2);

    um.undo(&mut g).unwrap(); // undo Alice knows Bob
    assert_eq!(g.len(), 1);

    um.undo(&mut g).unwrap(); // undo Alice name
    assert_eq!(g.len(), 0);

    um.redo(&mut g).unwrap(); // redo Alice name
    assert_eq!(g.len(), 1);
    assert!(g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));

    um.redo(&mut g).unwrap(); // redo Alice knows Bob
    assert_eq!(g.len(), 2);

    um.redo(&mut g).unwrap(); // redo Bob name
    assert_eq!(g.len(), 3);
}

#[test]
fn new_action_clears_redo_stack() {
    let (mut g, mut um) = setup_graph_with_undo();
    um.add_triple(&mut g, alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    um.add_triple(&mut g, alice(), FOAF_KNOWS, bob()).unwrap();

    um.undo(&mut g).unwrap(); // undo "knows"
    assert!(um.can_redo());

    // New action should clear redo stack
    um.add_triple(&mut g, bob(), FOAF_NAME, RdfTerm::literal("Bob"))
        .unwrap();
    assert!(!um.can_redo());

    let redo_result = um.redo(&mut g).unwrap();
    assert!(redo_result.is_none());
}

// ════════════════════════════════════════════════════════════════
//  can_undo / can_redo
// ════════════════════════════════════════════════════════════════

#[test]
fn can_undo_redo_flags() {
    let (mut g, mut um) = setup_graph_with_undo();
    assert!(!um.can_undo());
    assert!(!um.can_redo());

    um.add_triple(&mut g, alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    assert!(um.can_undo());
    assert!(!um.can_redo());

    um.undo(&mut g).unwrap();
    assert!(!um.can_undo());
    assert!(um.can_redo());

    um.redo(&mut g).unwrap();
    assert!(um.can_undo());
    assert!(!um.can_redo());
}

// ════════════════════════════════════════════════════════════════
//  Edge case: Undo Add when triple already removed externally
// ════════════════════════════════════════════════════════════════

#[test]
fn undo_add_when_already_removed() {
    let (mut g, mut um) = setup_graph_with_undo();
    um.add_triple(&mut g, alice(), FOAF_KNOWS, bob()).unwrap();

    // External removal (simulate remote replica removing it)
    g.remove_triple(&alice(), FOAF_KNOWS, &bob()).unwrap();
    assert_eq!(g.len(), 0);

    // Undo the add — triple is already gone, should be skipped gracefully
    let result = um.undo(&mut g).unwrap();
    assert!(result.is_none()); // no-op
    assert_eq!(g.len(), 0);
}

// ════════════════════════════════════════════════════════════════
//  Edge case: Undo Remove when triple already re-added externally
// ════════════════════════════════════════════════════════════════

#[test]
fn undo_remove_when_already_readded() {
    let (mut g, mut um) = setup_graph_with_undo();
    um.add_triple(&mut g, alice(), FOAF_KNOWS, bob()).unwrap();
    um.remove_triple(&mut g, &alice(), FOAF_KNOWS, &bob())
        .unwrap();
    assert_eq!(g.len(), 0);

    // External re-add (simulate remote replica adding the same triple)
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
    assert_eq!(g.len(), 1);

    // Undo the remove — triple already exists, should be skipped
    let result = um.undo(&mut g).unwrap();
    assert!(result.is_none()); // no-op
    assert_eq!(g.len(), 1);
}

// ════════════════════════════════════════════════════════════════
//  Shared vertex reuse after undo/redo
// ════════════════════════════════════════════════════════════════

#[test]
fn vertex_reused_on_redo_add() {
    let (mut g, mut um) = setup_graph_with_undo();
    um.add_triple(&mut g, alice(), FOAF_KNOWS, bob()).unwrap();

    // Vertices for alice and bob created
    let va_count_before = g.all_vertices_added().len();

    um.undo(&mut g).unwrap();
    um.redo(&mut g).unwrap();

    // After redo, no new vertices should be created (reusing existing ones)
    let va_count_after = g.all_vertices_added().len();
    assert_eq!(va_count_before, va_count_after);
    assert!(g.contains_triple(&alice(), FOAF_KNOWS, &bob()));
}

// ════════════════════════════════════════════════════════════════
//  Multi-replica broadcast
// ════════════════════════════════════════════════════════════════

#[test]
fn undo_broadcasts_to_replica() {
    let (mut replica_a, mut um) = setup_graph_with_undo();
    let mut replica_b = RdfGraph::new();

    // Replica A adds a triple, broadcast to B
    let op = um
        .add_triple(&mut replica_a, alice(), FOAF_KNOWS, bob())
        .unwrap();
    replica_b.apply_downstream(op).unwrap();
    assert_eq!(replica_a.len(), 1);
    assert_eq!(replica_b.len(), 1);

    // Replica A undoes, broadcast the inverse op to B
    let undo_op = um.undo(&mut replica_a).unwrap().unwrap();
    replica_b.apply_downstream(undo_op).unwrap();
    assert_eq!(replica_a.len(), 0);
    assert_eq!(replica_b.len(), 0);
}

#[test]
fn redo_broadcasts_to_replica() {
    let (mut replica_a, mut um) = setup_graph_with_undo();
    let mut replica_b = RdfGraph::new();

    // A: add → broadcast to B
    let op = um
        .add_triple(&mut replica_a, alice(), FOAF_KNOWS, bob())
        .unwrap();
    replica_b.apply_downstream(op).unwrap();

    // A: undo → broadcast to B
    let undo_op = um.undo(&mut replica_a).unwrap().unwrap();
    replica_b.apply_downstream(undo_op).unwrap();
    assert_eq!(replica_b.len(), 0);

    // A: redo → broadcast to B
    let redo_op = um.redo(&mut replica_a).unwrap().unwrap();
    replica_b.apply_downstream(redo_op).unwrap();
    assert_eq!(replica_a.len(), 1);
    assert_eq!(replica_b.len(), 1);
}

// ════════════════════════════════════════════════════════════════
//  Undo multiple removes then redo
// ════════════════════════════════════════════════════════════════

#[test]
fn undo_multiple_removes() {
    let (mut g, mut um) = setup_graph_with_undo();
    um.add_triple(&mut g, alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    um.add_triple(&mut g, alice(), FOAF_KNOWS, bob()).unwrap();
    assert_eq!(g.len(), 2);

    um.remove_triple(&mut g, &alice(), FOAF_NAME, &RdfTerm::literal("Alice"))
        .unwrap();
    um.remove_triple(&mut g, &alice(), FOAF_KNOWS, &bob())
        .unwrap();
    assert_eq!(g.len(), 0);

    um.undo(&mut g).unwrap(); // undo remove knows
    assert_eq!(g.len(), 1);
    assert!(g.contains_triple(&alice(), FOAF_KNOWS, &bob()));

    um.undo(&mut g).unwrap(); // undo remove name
    assert_eq!(g.len(), 2);

    um.redo(&mut g).unwrap(); // redo remove name
    assert_eq!(g.len(), 1);
    assert!(g.contains_triple(&alice(), FOAF_KNOWS, &bob()));
    assert!(!g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
}

// ════════════════════════════════════════════════════════════════
//  Interleaved add and remove
// ════════════════════════════════════════════════════════════════

#[test]
fn interleaved_add_remove_undo() {
    let (mut g, mut um) = setup_graph_with_undo();

    um.add_triple(&mut g, alice(), FOAF_KNOWS, bob()).unwrap(); // action 1
    um.add_triple(&mut g, bob(), FOAF_KNOWS, charlie()).unwrap(); // action 2
    um.remove_triple(&mut g, &alice(), FOAF_KNOWS, &bob())
        .unwrap(); // action 3
    assert_eq!(g.len(), 1); // only bob→charlie

    um.undo(&mut g).unwrap(); // undo action 3 → re-add alice→bob
    assert_eq!(g.len(), 2);

    um.undo(&mut g).unwrap(); // undo action 2 → remove bob→charlie
    assert_eq!(g.len(), 1);
    assert!(g.contains_triple(&alice(), FOAF_KNOWS, &bob()));
    assert!(!g.contains_triple(&bob(), FOAF_KNOWS, &charlie()));

    um.undo(&mut g).unwrap(); // undo action 1 → remove alice→bob
    assert_eq!(g.len(), 0);
}

// ════════════════════════════════════════════════════════════════
//  CRDT state growth
// ════════════════════════════════════════════════════════════════

#[test]
fn undo_redo_grows_crdt_state() {
    let (mut g, mut um) = setup_graph_with_undo();
    um.add_triple(&mut g, alice(), FOAF_KNOWS, bob()).unwrap();
    // E_A has 1 entry
    assert_eq!(g.all_edges_added().len(), 1);
    assert_eq!(g.all_edges_removed().len(), 0);

    um.undo(&mut g).unwrap();
    // E_A: 1, E_R: 1 (the undo added a RemoveEdge)
    assert_eq!(g.all_edges_added().len(), 1);
    assert_eq!(g.all_edges_removed().len(), 1);

    um.redo(&mut g).unwrap();
    // E_A: 2 (new AddEdge for the redo), E_R: 1
    assert_eq!(g.all_edges_added().len(), 2);
    assert_eq!(g.all_edges_removed().len(), 1);
}

// ════════════════════════════════════════════════════════════════
//  Redo after external edit (redo should still work)
// ════════════════════════════════════════════════════════════════

#[test]
fn redo_after_external_edit() {
    let (mut g, mut um) = setup_graph_with_undo();
    um.add_triple(&mut g, alice(), FOAF_KNOWS, bob()).unwrap();
    um.undo(&mut g).unwrap();
    assert_eq!(g.len(), 0);

    // External edit: add a different triple
    g.add_triple(charlie(), FOAF_NAME, RdfTerm::literal("Charlie"))
        .unwrap();
    assert_eq!(g.len(), 1);

    // Redo should still add alice→bob back
    um.redo(&mut g).unwrap();
    assert_eq!(g.len(), 2);
    assert!(g.contains_triple(&alice(), FOAF_KNOWS, &bob()));
}

// ════════════════════════════════════════════════════════════════
//  Duplicate redo skip
// ════════════════════════════════════════════════════════════════

#[test]
fn redo_remove_when_triple_already_gone() {
    let (mut g, mut um) = setup_graph_with_undo();
    um.add_triple(&mut g, alice(), FOAF_KNOWS, bob()).unwrap();
    um.remove_triple(&mut g, &alice(), FOAF_KNOWS, &bob())
        .unwrap();
    um.undo(&mut g).unwrap(); // re-added
    assert_eq!(g.len(), 1);

    // External remove before redo
    g.remove_triple(&alice(), FOAF_KNOWS, &bob()).unwrap();
    assert_eq!(g.len(), 0);

    // Redo the remove — triple already gone, should skip
    let result = um.redo(&mut g).unwrap();
    assert!(result.is_none());
    assert_eq!(g.len(), 0);
}

// ════════════════════════════════════════════════════════════════
//  Multi-replica full undo/redo cycle
// ════════════════════════════════════════════════════════════════

#[test]
fn full_multi_replica_undo_redo_cycle() {
    let (mut ra, mut um) = setup_graph_with_undo();
    let mut rb = RdfGraph::new();

    // A: add triple, sync to B
    let op1 = um.add_triple(&mut ra, alice(), FOAF_KNOWS, bob()).unwrap();
    rb.apply_downstream(op1).unwrap();

    // A: add another triple, sync to B
    let op2 = um
        .add_triple(&mut ra, bob(), FOAF_NAME, RdfTerm::literal("Bob"))
        .unwrap();
    rb.apply_downstream(op2).unwrap();
    assert_eq!(ra.len(), 2);
    assert_eq!(rb.len(), 2);

    // A: undo (remove bob name), sync to B
    let undo1 = um.undo(&mut ra).unwrap().unwrap();
    rb.apply_downstream(undo1).unwrap();
    assert_eq!(ra.len(), 1);
    assert_eq!(rb.len(), 1);

    // A: undo (remove alice knows bob), sync to B
    let undo2 = um.undo(&mut ra).unwrap().unwrap();
    rb.apply_downstream(undo2).unwrap();
    assert_eq!(ra.len(), 0);
    assert_eq!(rb.len(), 0);

    // A: redo (re-add alice knows bob), sync to B
    let redo1 = um.redo(&mut ra).unwrap().unwrap();
    rb.apply_downstream(redo1).unwrap();
    assert_eq!(ra.len(), 1);
    assert_eq!(rb.len(), 1);

    // A: redo (re-add bob name), sync to B
    let redo2 = um.redo(&mut ra).unwrap().unwrap();
    rb.apply_downstream(redo2).unwrap();
    assert_eq!(ra.len(), 2);
    assert_eq!(rb.len(), 2);
}

// ════════════════════════════════════════════════════════════════
//  Transactional (batched) undo / redo
// ════════════════════════════════════════════════════════════════

#[test]
fn transactional_duplicate_triples_survive_undo_redo() {
    let mut g = RdfGraph::new();
    let mut um = TransactionalUndoManager::new();
    let before = g.clone();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
    g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
    assert_eq!(g.len(), 2);
    assert!(um.commit_diff("add two copies", &before, &g));

    um.undo(&mut g).unwrap().expect("transaction present");
    assert_eq!(g.len(), 0);

    um.redo(&mut g).unwrap().expect("transaction present");
    assert_eq!(g.len(), 2);
}

#[test]
fn transactional_diff_counts_existing_and_removed_copies() {
    for before_count in 0..=3 {
        for after_count in 0..=3 {
            let mut g = RdfGraph::new();
            let mut um = TransactionalUndoManager::new();
            for _ in 0..before_count {
                g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
            }
            let before = g.clone();
            for _ in before_count..after_count {
                g.add_triple(alice(), FOAF_KNOWS, bob()).unwrap();
            }
            for _ in after_count..before_count {
                g.remove_triple(&alice(), FOAF_KNOWS, &bob()).unwrap();
            }
            assert_eq!(
                um.commit_diff("change copies", &before, &g),
                before_count != after_count
            );
            um.undo(&mut g).unwrap();
            assert_eq!(g.len(), before_count);
            um.redo(&mut g).unwrap();
            assert_eq!(g.len(), after_count);
        }
    }
}

#[test]
fn transactional_commit_diff_records_added_and_removed_triples() {
    let mut g = RdfGraph::new();
    let mut um = TransactionalUndoManager::new();

    // Seed one triple so the first "user edit" both adds and removes.
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();

    let before = g.clone();
    g.remove_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice"))
        .unwrap();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alicia"))
        .unwrap();
    g.add_triple(bob(), FOAF_AGE, RdfTerm::literal("30"))
        .unwrap();

    assert!(um.commit_diff("rename alice + add bob age", &before, &g));
    assert!(um.can_undo());
    assert!(!um.can_redo());
    assert_eq!(um.peek_undo_label(), Some("rename alice + add bob age"));

    // One undo step must restore *all* three triple-level effects.
    let undone = um.undo(&mut g).unwrap().expect("transaction present");
    assert_eq!(undone.label, "rename alice + add bob age");
    assert!(g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
    assert!(!g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alicia")));
    assert!(!g.contains_triple(&bob(), FOAF_AGE, &RdfTerm::literal("30")));

    // One redo step re-applies all three effects.
    let redone = um.redo(&mut g).unwrap().expect("transaction present");
    assert_eq!(redone.label, "rename alice + add bob age");
    assert!(!g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice")));
    assert!(g.contains_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alicia")));
    assert!(g.contains_triple(&bob(), FOAF_AGE, &RdfTerm::literal("30")));
}

#[test]
fn transactional_commit_diff_on_empty_change_records_nothing() {
    let mut g = RdfGraph::new();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();

    let before = g.clone();
    let after = g.clone();
    let mut um = TransactionalUndoManager::new();
    assert!(!um.commit_diff("noop", &before, &after));
    assert!(!um.can_undo());
}

#[test]
fn transactional_fresh_commit_clears_redo() {
    let mut g = RdfGraph::new();
    let mut um = TransactionalUndoManager::new();

    let before = g.clone();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    assert!(um.commit_diff("add alice", &before, &g));

    um.undo(&mut g).unwrap();
    assert!(um.can_redo());

    // A new transaction must invalidate the redo stack.
    let before = g.clone();
    g.add_triple(bob(), FOAF_NAME, RdfTerm::literal("Bob"))
        .unwrap();
    assert!(um.commit_diff("add bob", &before, &g));
    assert!(!um.can_redo());
}

#[test]
fn transactional_history_limit_drops_oldest() {
    let mut g = RdfGraph::new();
    let mut um = TransactionalUndoManager::with_limit(2);
    for i in 0..5 {
        let before = g.clone();
        g.add_triple(
            RdfTerm::iri(format!("http://example.org/n{i}")),
            FOAF_NAME,
            RdfTerm::literal(format!("n{i}")),
        )
        .unwrap();
        assert!(um.commit_diff(format!("add n{i}"), &before, &g));
    }
    // Only the most recent two transactions survive.
    assert_eq!(um.peek_undo_label(), Some("add n4"));
    um.undo(&mut g).unwrap();
    assert_eq!(um.peek_undo_label(), Some("add n3"));
    um.undo(&mut g).unwrap();
    assert!(!um.can_undo());
}

#[test]
fn transactional_undo_is_idempotent_when_inverse_is_noop() {
    // If the graph already lacks the added triple (e.g. a peer
    // converged on the same removal), undo must still pop the
    // transaction without erroring.
    let mut g = RdfGraph::new();
    let mut um = TransactionalUndoManager::new();
    let before = g.clone();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    assert!(um.commit_diff("add alice", &before, &g));

    // Remove the triple out-of-band before undoing.
    g.remove_triple(&alice(), FOAF_NAME, &RdfTerm::literal("Alice"))
        .unwrap();
    let undone = um.undo(&mut g).unwrap().expect("transaction popped");
    assert_eq!(undone.label, "add alice");
    assert!(
        undone.ops.is_empty(),
        "no compensating op when already removed"
    );
}

#[test]
fn transactional_peek_shows_a_transaction_without_taking_it() {
    let mut g = RdfGraph::new();
    let mut um = TransactionalUndoManager::new();
    assert!(um.peek_undo().is_none());
    let before = g.clone();
    g.add_triple(alice(), FOAF_NAME, RdfTerm::literal("Alice"))
        .unwrap();
    assert!(um.commit_diff("add alice", &before, &g));

    let tx = um.peek_undo().expect("one transaction");
    assert_eq!(tx.label(), "add alice");
    let forward: Vec<_> = tx
        .forward()
        .iter()
        .map(|action| (*action.kind(), action.object().clone()))
        .collect();
    assert_eq!(
        forward,
        [(crdf::ActionKind::Add, RdfTerm::literal("Alice"))]
    );
    // Looking moved nothing.
    assert!(um.can_undo() && !um.can_redo());
    um.undo(&mut g).unwrap();
    assert!(um.peek_undo().is_none());
    assert_eq!(um.peek_redo().map(|tx| tx.label()), Some("add alice"));
}
