use std::collections::HashMap;

use crate::error::CrdfError;
use crate::graph::{RdfGraph, RdfOperation};
use crate::term::RdfTerm;
use crate::triple::Triple;

/// Describes the kind of triple-level action for undo/redo tracking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    Add,
    Remove,
}

/// A recorded triple-level action that can be undone or redone.
///
/// Public so it can appear inside the generic
/// [`crdt_graph::GraphTransaction`] alias surfaced as [`GraphTransaction`];
/// fields stay private since callers should construct actions through
/// [`UndoManager::add_triple`] / [`UndoManager::remove_triple`] (per-
/// triple manager) or [`TransactionalUndoManager::commit_diff`] (batched
/// manager) rather than building them by hand.
#[derive(Clone, Debug)]
pub struct TripleAction {
    kind: ActionKind,
    subject: RdfTerm,
    predicate: String,
    object: RdfTerm,
}

impl TripleAction {
    /// The kind of triple-level operation (add or remove).
    pub fn kind(&self) -> &ActionKind {
        &self.kind
    }

    /// The triple's subject.
    pub fn subject(&self) -> &RdfTerm {
        &self.subject
    }

    /// The triple's predicate IRI.
    pub fn predicate(&self) -> &str {
        &self.predicate
    }

    /// The triple's object.
    pub fn object(&self) -> &RdfTerm {
        &self.object
    }

    /// Execute this action on the graph, returning the broadcastable operation.
    /// Counted transactions allow duplicate additions; single-action history
    /// skips additions already present. Both skip removals already absent.
    fn execute(
        &self,
        graph: &mut RdfGraph,
        allow_duplicate_add: bool,
    ) -> Result<Option<RdfOperation>, CrdfError> {
        match self.kind {
            ActionKind::Add => {
                if !allow_duplicate_add
                    && graph.contains_triple(&self.subject, &self.predicate, &self.object)
                {
                    return Ok(None);
                }
                let op = graph.add_triple(
                    self.subject.clone(),
                    self.predicate.clone(),
                    self.object.clone(),
                )?;
                Ok(Some(op))
            }
            ActionKind::Remove => {
                if !graph.contains_triple(&self.subject, &self.predicate, &self.object) {
                    return Ok(None);
                }
                let op = graph.remove_triple(&self.subject, &self.predicate, &self.object)?;
                Ok(Some(op))
            }
        }
    }

    /// Return the inverse action (Add ↔ Remove with same triple).
    fn inverse(&self) -> Self {
        Self {
            kind: match self.kind {
                ActionKind::Add => ActionKind::Remove,
                ActionKind::Remove => ActionKind::Add,
            },
            subject: self.subject.clone(),
            predicate: self.predicate.clone(),
            object: self.object.clone(),
        }
    }
}

/// Tracks triple-level operations for undo/redo with CRDT-safe compensating
/// operations.
///
/// Each undo produces the inverse operation (Add → Remove, Remove → Add) and
/// returns an [`RdfOperation`] suitable for broadcasting to remote replicas.
///
/// # Limitations
///
/// Because the underlying 2P2P-Graph CRDT is append-only, undo/redo creates
/// *new* CRDT operations rather than rolling back existing ones. This means
/// the CRDT state sets grow monotonically.
pub struct UndoManager {
    undo_stack: Vec<TripleAction>,
    redo_stack: Vec<TripleAction>,
}

impl Default for UndoManager {
    fn default() -> Self {
        Self::new()
    }
}

impl UndoManager {
    /// Creates a new, empty undo manager.
    pub fn new() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Returns `true` if there is an action that can be undone.
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Returns `true` if there is an action that can be redone.
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Clears both undo and redo stacks.
    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    /// Adds a triple via the graph, recording the action for undo.
    ///
    /// Clears the redo stack (standard undo/redo semantics).
    pub fn add_triple(
        &mut self,
        graph: &mut RdfGraph,
        subject: RdfTerm,
        predicate: impl Into<String>,
        object: RdfTerm,
    ) -> Result<RdfOperation, CrdfError> {
        let predicate = predicate.into();
        let op = graph.add_triple(subject.clone(), predicate.clone(), object.clone())?;

        self.undo_stack.push(TripleAction {
            kind: ActionKind::Add,
            subject,
            predicate,
            object,
        });
        self.redo_stack.clear();

        Ok(op)
    }

    /// Removes a triple via the graph, recording the action for undo.
    ///
    /// Clears the redo stack (standard undo/redo semantics).
    pub fn remove_triple(
        &mut self,
        graph: &mut RdfGraph,
        subject: &RdfTerm,
        predicate: &str,
        object: &RdfTerm,
    ) -> Result<RdfOperation, CrdfError> {
        let op = graph.remove_triple(subject, predicate, object)?;

        self.undo_stack.push(TripleAction {
            kind: ActionKind::Remove,
            subject: subject.clone(),
            predicate: predicate.to_owned(),
            object: object.clone(),
        });
        self.redo_stack.clear();

        Ok(op)
    }

    /// Undoes the last action, returning the broadcastable compensating
    /// operation.
    ///
    /// Returns `Ok(None)` if the undo stack is empty or the inverse action is
    /// a no-op (e.g., an external operation already achieved the same effect).
    pub fn undo(&mut self, graph: &mut RdfGraph) -> Result<Option<RdfOperation>, CrdfError> {
        let Some(action) = self.undo_stack.pop() else {
            return Ok(None);
        };

        let inverse = action.inverse();
        let result = inverse.execute(graph, false)?;

        // Always push the original action to redo so the user can redo even
        // if the inverse was a no-op this time.
        self.redo_stack.push(action);

        Ok(result)
    }

    /// Redoes the last undone action, returning the broadcastable operation.
    ///
    /// Returns `Ok(None)` if the redo stack is empty or the action is a no-op.
    pub fn redo(&mut self, graph: &mut RdfGraph) -> Result<Option<RdfOperation>, CrdfError> {
        let Some(action) = self.redo_stack.pop() else {
            return Ok(None);
        };

        let result = action.execute(graph, false)?;

        self.undo_stack.push(action);

        Ok(result)
    }
}

/// Default cap on the number of transactions retained per stack.
///
/// Re-export of [`crdt_graph::DEFAULT_TRANSACTION_HISTORY_LIMIT`] so
/// callers don't have to depend on `crdt-graph` directly.
pub use crdt_graph::DEFAULT_TRANSACTION_HISTORY_LIMIT;

/// A batched user-level edit recorded for undo/redo.
///
/// Thin alias over [`crdt_graph::GraphTransaction`] parameterised by
/// the per-triple action type that this crate's RDF graph uses. One
/// transaction bundles every triple-level action produced by a single
/// logical edit, so one `undo()` call reverses the whole edit in one
/// step (which is what users expect of "Ctrl+Z").
pub type GraphTransaction = crdt_graph::GraphTransaction<TripleAction>;

/// Tracks **batched** triple-level operations for undo/redo, one
/// transaction per user edit.
///
/// The caller drives the lifecycle:
///   1. Snapshot the graph before the edit (`graph.clone()`).
///   2. Apply mutations through the regular `RdfGraph` API.
///   3. Call [`commit_diff`](Self::commit_diff) with the pre-edit
///      snapshot and the current graph; the manager computes the diff
///      and pushes one transaction onto the undo stack.
///
/// [`undo`](Self::undo) / [`redo`](Self::redo) replay the recorded
/// transaction as a single batch and produce broadcastable
/// [`RdfOperation`]s for collaboration.
///
/// # Layering
///
/// Storage and history management live in
/// [`crdt_graph::TransactionStack`] — that part is generic across any
/// CRDT-graph user. This manager adds RDF-specific concerns: diffing
/// `RdfGraph` snapshots and executing the inverse via the high-level
/// `add_triple` / `remove_triple` API.
///
/// # Why diff-based?
///
/// A projection may rebuild its canonical `RdfGraph` from a typed
/// in-memory cache via a snapshot helper, rather than calling
/// `add_triple` / `remove_triple` per change. Recording inverses by
/// diffing snapshots is therefore the natural fit: it captures exactly
/// what the canonical document gained and lost, regardless of how many
/// internal mutations the typed cache performed.
pub struct TransactionalUndoManager {
    stack: crdt_graph::TransactionStack<TripleAction>,
}

impl Default for TransactionalUndoManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TransactionalUndoManager {
    /// Creates a new manager with the default history limit
    /// ([`DEFAULT_TRANSACTION_HISTORY_LIMIT`]).
    pub fn new() -> Self {
        Self {
            stack: crdt_graph::TransactionStack::new(),
        }
    }

    /// Creates a new manager that retains at most `limit` transactions
    /// per stack. A `limit` of zero means **no limit** — every
    /// transaction is kept, as `crdt_graph::TransactionStack` does.
    pub fn with_limit(limit: usize) -> Self {
        Self {
            stack: crdt_graph::TransactionStack::with_limit(limit),
        }
    }

    /// Diff `before` against `after` and record the diff as one undo
    /// transaction. Pushing a transaction clears the redo stack
    /// (standard linear-history semantics).
    ///
    /// Returns `true` when a non-empty transaction was recorded.
    pub fn commit_diff(
        &mut self,
        label: impl Into<String>,
        before: &RdfGraph,
        after: &RdfGraph,
    ) -> bool {
        let tx = diff_to_transaction(label.into(), before, after);
        self.stack.push(tx)
    }

    /// Pushes a pre-built transaction onto the undo stack. Useful when
    /// the caller builds transactions outside of `commit_diff` (e.g.
    /// replays from a serialized history).
    pub fn push_transaction(&mut self, tx: GraphTransaction) {
        self.stack.push(tx);
    }

    /// Returns `true` when at least one transaction can be undone.
    pub fn can_undo(&self) -> bool {
        self.stack.can_undo()
    }

    /// Returns `true` when at least one transaction can be redone.
    pub fn can_redo(&self) -> bool {
        self.stack.can_redo()
    }

    /// Discards both stacks. Use when loading a fresh project — undo
    /// should never cross project-load boundaries.
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    /// Pop the most recent undo transaction without applying it.
    ///
    /// Intended for the GUI's coalesce-window logic: when a drag-frame
    /// edit arrives that should be folded into the prior gesture, the
    /// caller drops the prior transaction, applies the new frame, and
    /// commits a fresh transaction spanning the gesture from the
    /// original pre-drag snapshot. The redo stack is **not** affected
    /// (this is not an undo).
    pub fn drop_last_undo(&mut self) -> Option<GraphTransaction> {
        self.stack.drop_last_undo()
    }

    /// Labels of every undoable transaction, oldest first.
    pub fn undo_labels(&self) -> Vec<&str> {
        self.stack.undo_labels().collect()
    }

    /// Labels of every redoable transaction; the **last** entry is the
    /// next transaction a redo would re-apply.
    pub fn redo_labels(&self) -> Vec<&str> {
        self.stack.redo_labels().collect()
    }

    /// The transaction the next [`undo`](Self::undo) would reverse, left
    /// where it is. Its [`forward`](GraphTransaction::forward) actions say
    /// what the edit did, so a caller can check that the graph still holds
    /// that outcome before reversing it — undoing an edit that something
    /// else has since changed would put the old value beside the new one.
    pub fn peek_undo(&self) -> Option<&GraphTransaction> {
        self.stack.peek_undo()
    }

    /// The transaction the next [`redo`](Self::redo) would re-apply, left
    /// where it is.
    pub fn peek_redo(&self) -> Option<&GraphTransaction> {
        self.stack.peek_redo()
    }

    /// Label of the next transaction that would be undone.
    pub fn peek_undo_label(&self) -> Option<&str> {
        self.stack.peek_undo_label()
    }

    /// Label of the next transaction that would be redone.
    pub fn peek_redo_label(&self) -> Option<&str> {
        self.stack.peek_redo_label()
    }

    /// Number of transactions currently on the undo stack.
    pub fn undo_len(&self) -> usize {
        self.stack.undo_len()
    }

    /// Number of transactions currently on the redo stack.
    pub fn redo_len(&self) -> usize {
        self.stack.redo_len()
    }

    /// Undoes the most recent transaction. The transaction's inverse
    /// (remove every added triple, then re-add every removed one) is
    /// applied to `graph`. Any actual CRDT operations produced are
    /// collected and returned so the caller can broadcast them.
    ///
    /// Returns `Ok(None)` if the undo stack is empty.
    pub fn undo(&mut self, graph: &mut RdfGraph) -> Result<Option<UndoneTransaction>, CrdfError> {
        let Some(tx) = self.stack.pop_undo() else {
            return Ok(None);
        };

        let mut ops = Vec::new();
        // The transaction's `inverse` slice already encodes the
        // compensating ops (Remove for each added triple, Add for each
        // removed one) — see `diff_to_transaction`.
        for action in tx.inverse() {
            if let Some(op) = action.execute(graph, true)? {
                ops.push(op);
            }
        }

        let label = tx.label().to_string();
        self.stack.push_to_redo(tx);

        Ok(Some(UndoneTransaction { label, ops }))
    }

    /// Redoes the most recently undone transaction (forward direction).
    /// Returns `Ok(None)` if the redo stack is empty.
    pub fn redo(&mut self, graph: &mut RdfGraph) -> Result<Option<UndoneTransaction>, CrdfError> {
        let Some(tx) = self.stack.pop_redo() else {
            return Ok(None);
        };

        let mut ops = Vec::new();
        // The transaction's `forward` slice replays the original edit
        // (Add for each added triple, Remove for each removed one).
        for action in tx.forward() {
            if let Some(op) = action.execute(graph, true)? {
                ops.push(op);
            }
        }

        let label = tx.label().to_string();
        self.stack.push_to_undo(tx);

        Ok(Some(UndoneTransaction { label, ops }))
    }
}

/// Result of [`TransactionalUndoManager::undo`] /
/// [`redo`](TransactionalUndoManager::redo).
#[derive(Debug)]
pub struct UndoneTransaction {
    /// Label of the transaction that was just applied (the original
    /// user-edit label, regardless of direction).
    pub label: String,
    /// CRDT operations actually produced. May be empty when the inverse
    /// was a no-op against the current graph state (e.g. a concurrent
    /// peer already converged to the same value).
    pub ops: Vec<RdfOperation>,
}

fn diff_to_transaction(label: String, before: &RdfGraph, after: &RdfGraph) -> GraphTransaction {
    let counts = |graph: &RdfGraph| {
        let mut counts: HashMap<Triple, usize> = HashMap::new();
        for triple in graph.triples() {
            *counts.entry(triple).or_default() += 1;
        }
        counts
    };
    let before_counts = counts(before);
    let after_counts = counts(after);

    // Each add creates one edge and each remove removes one matching edge,
    // so record one action per copy gained or lost. We pre-bake the lists the
    // generic stack needs:
    //   forward = "do the edit again" (Add for added, Remove for removed)
    //   inverse = "reverse the edit"  (Remove for added, Add for removed)
    let mut forward = Vec::new();
    let mut inverse = Vec::new();

    for (triple, count) in &after_counts {
        for _ in 0..count.saturating_sub(*before_counts.get(triple).unwrap_or(&0)) {
            forward.push(TripleAction {
                kind: ActionKind::Add,
                subject: triple.subject.clone(),
                predicate: triple.predicate.clone(),
                object: triple.object.clone(),
            });
            inverse.push(TripleAction {
                kind: ActionKind::Remove,
                subject: triple.subject.clone(),
                predicate: triple.predicate.clone(),
                object: triple.object.clone(),
            });
        }
    }
    for (triple, count) in &before_counts {
        for _ in 0..count.saturating_sub(*after_counts.get(triple).unwrap_or(&0)) {
            forward.push(TripleAction {
                kind: ActionKind::Remove,
                subject: triple.subject.clone(),
                predicate: triple.predicate.clone(),
                object: triple.object.clone(),
            });
            inverse.push(TripleAction {
                kind: ActionKind::Add,
                subject: triple.subject.clone(),
                predicate: triple.predicate.clone(),
                object: triple.object.clone(),
            });
        }
    }

    GraphTransaction::new(label, forward, inverse)
}
