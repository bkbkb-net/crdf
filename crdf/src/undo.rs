use crate::error::CrdfError;
use crate::graph::{RdfGraph, RdfOperation};
use crate::term::RdfTerm;

/// Describes the kind of triple-level action for undo/redo tracking.
#[derive(Clone, Debug)]
enum ActionKind {
    Add,
    Remove,
}

/// A recorded triple-level action that can be undone or redone.
#[derive(Clone, Debug)]
struct TripleAction {
    kind: ActionKind,
    subject: RdfTerm,
    predicate: String,
    object: RdfTerm,
}

impl TripleAction {
    /// Execute this action on the graph, returning the broadcastable operation.
    /// Returns `Ok(None)` if the action is a no-op (e.g., triple already present/absent).
    fn execute(&self, graph: &mut RdfGraph) -> Result<Option<RdfOperation>, CrdfError> {
        match self.kind {
            ActionKind::Add => {
                if graph.contains_triple(&self.subject, &self.predicate, &self.object) {
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
        let result = inverse.execute(graph)?;

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

        let result = action.execute(graph)?;

        self.undo_stack.push(action);

        Ok(result)
    }
}
