//! Imperative circuit editing against a live CRDT graph.
//!
//! [`CircuitEditor`] is the write-side counterpart of [`crate::crdf_io`]:
//! every operation is expressed directly as triple additions/removals
//! on the caller's [`RdfGraph`], so the CRDT operation history is
//! preserved and concurrent edits merge conflict-free. (Loading an
//! asset into structs and re-saving it would create fresh history and
//! duplicate resources — use the editor for edits, the loader for
//! compilation.)
//!
//! Every local mutation's [`crdf::RdfOperation`] is retained by the editor in
//! causal order. Replicating callers can broadcast [`CircuitEditor::operations`]
//! or periodically drain the batch with [`CircuitEditor::take_operations`].
//!
//! Editing rules enforced by this API shape:
//!
//! - **Wires are immutable**: there is no endpoint mutation, only
//!   [`CircuitEditor::remove_wire`] + [`CircuitEditor::add_wire`], so a
//!   merge can never see a half-rewired connection.
//! - **Removals cascade locally**: removing a cell or port also
//!   removes every wire referencing it, keeping the graph free of
//!   dangling endpoint triples. Removing a module removes its members.
//! - **Functional properties are replaced atomically**: setters remove
//!   the existing value triples before adding the new one.
//!
//! The editor performs no structural validation beyond that: a merged
//! or in-progress graph may be temporarily invalid, and
//! [`crate::compile`] remains the single source of truth. Callers keep
//! the last valid [`crate::CompiledCircuit`] while edits are invalid.
//!
//! Rule-based rewriting (crdf-dpo) is intentionally *not* used here:
//! interactive edits address resources by known id, where pattern
//! matching adds cost without safety. DPO rules remain the right tool
//! for declarative transformations layered on top (e.g. "replace every
//! inline XOR with an `std.xor` instance").

use crdf::{RdfGraph, RdfOperation, RdfTerm};

use crate::crdf_io::{CircuitCrdfError, iri};
use crate::model::{
    AssetId, CellId, CellKind, Endpoint, ModuleId, PortDirection, PortId, PortRef, WireId,
};
use crate::vocab::*;

type Result<T> = std::result::Result<T, CircuitCrdfError>;

/// An editing session over a live graph. Cheap to construct; hold it
/// only for the duration of an edit batch.
pub struct CircuitEditor<'g> {
    graph: &'g mut RdfGraph,
    operations: Vec<RdfOperation>,
}

impl<'g> CircuitEditor<'g> {
    pub fn new(graph: &'g mut RdfGraph) -> Self {
        Self {
            graph,
            operations: Vec::new(),
        }
    }

    /// Operations produced by this editing session, in causal order.
    ///
    /// Broadcast clones of these operations to other replicas, or use
    /// [`CircuitEditor::take_operations`] to move them out without cloning.
    pub fn operations(&self) -> &[RdfOperation] {
        &self.operations
    }

    /// Takes all operations produced so far, leaving the session ready
    /// to collect the next broadcast batch.
    pub fn take_operations(&mut self) -> Vec<RdfOperation> {
        std::mem::take(&mut self.operations)
    }

    /// Finishes the session and returns every operation not previously
    /// taken with [`CircuitEditor::take_operations`].
    pub fn into_operations(self) -> Vec<RdfOperation> {
        self.operations
    }

    // ------------------------------------------------------- assets

    /// Creates a new asset envelope referencing `root`.
    pub fn create_asset(&mut self, root: ModuleId, ticks_per_step: u8) -> Result<AssetId> {
        let id = AssetId::new_random();
        let subject = iri(ASSET_PREFIX, id.0);
        self.add_triple(subject.clone(), RDF_TYPE, RdfTerm::iri(TYPE_ASSET))?;
        self.add_triple(
            subject.clone(),
            PRED_SCHEMA_VERSION,
            RdfTerm::literal(crate::crdf_io::CIRCUIT_CRDF_SCHEMA_VERSION.to_string()),
        )?;
        self.add_triple(
            subject.clone(),
            PRED_ROOT_MODULE,
            iri(MODULE_PREFIX, root.0),
        )?;
        self.add_triple(
            subject,
            PRED_TICKS_PER_STEP,
            RdfTerm::literal(ticks_per_step.to_string()),
        )?;
        Ok(id)
    }

    pub fn set_ticks_per_step(&mut self, asset: AssetId, ticks_per_step: u8) -> Result<()> {
        self.replace_property(
            iri(ASSET_PREFIX, asset.0),
            PRED_TICKS_PER_STEP,
            RdfTerm::literal(ticks_per_step.to_string()),
        )
    }

    pub fn set_asset_root(&mut self, asset: AssetId, root: ModuleId) -> Result<()> {
        self.replace_property(
            iri(ASSET_PREFIX, asset.0),
            PRED_ROOT_MODULE,
            iri(MODULE_PREFIX, root.0),
        )
    }

    /// Removes the asset envelope. Module definitions are shared and
    /// stay in the graph (garbage collection is a caller policy).
    pub fn remove_asset(&mut self, asset: AssetId) -> Result<()> {
        self.remove_subject(iri(ASSET_PREFIX, asset.0))
    }

    // ------------------------------------------------------ modules

    pub fn create_module(&mut self, name: &str) -> Result<ModuleId> {
        let id = ModuleId::new_random();
        let subject = iri(MODULE_PREFIX, id.0);
        self.add_triple(subject.clone(), RDF_TYPE, RdfTerm::iri(TYPE_MODULE))?;
        self.add_triple(subject, PRED_MODULE_NAME, RdfTerm::literal(name))?;
        Ok(id)
    }

    pub fn set_module_name(&mut self, module: ModuleId, name: &str) -> Result<()> {
        self.replace_property(
            iri(MODULE_PREFIX, module.0),
            PRED_MODULE_NAME,
            RdfTerm::literal(name),
        )
    }

    /// Removes a module and all of its ports, cells and wires.
    /// Instances of the module elsewhere become dangling references
    /// that the loader/compiler will report.
    pub fn remove_module(&mut self, module: ModuleId) -> Result<()> {
        let module_iri = iri(MODULE_PREFIX, module.0);
        for predicate in [PRED_PORT_OF, PRED_CELL_OF, PRED_WIRE_OF] {
            for member in self
                .graph
                .subjects_for_predicate_object(predicate, &module_iri)
            {
                self.remove_subject(member)?;
            }
        }
        self.remove_subject(module_iri)
    }

    // ------------------------------------------------------ members

    pub fn add_port(
        &mut self,
        module: ModuleId,
        name: &str,
        direction: PortDirection,
    ) -> Result<PortId> {
        let id = PortId::new_random();
        let subject = iri(PORT_PREFIX, id.0);
        self.add_triple(subject.clone(), RDF_TYPE, RdfTerm::iri(TYPE_PORT))?;
        self.add_triple(subject.clone(), PRED_PORT_OF, iri(MODULE_PREFIX, module.0))?;
        self.add_triple(subject.clone(), PRED_PORT_NAME, RdfTerm::literal(name))?;
        let direction = match direction {
            PortDirection::Input => DIRECTION_IN,
            PortDirection::Output => DIRECTION_OUT,
        };
        self.add_triple(subject, PRED_PORT_DIRECTION, RdfTerm::literal(direction))?;
        Ok(id)
    }

    /// Renames a port without changing its stable identity or any wire
    /// endpoints that reference it.
    pub fn set_port_name(&mut self, port: PortId, name: &str) -> Result<()> {
        self.replace_property(
            iri(PORT_PREFIX, port.0),
            PRED_PORT_NAME,
            RdfTerm::literal(name),
        )
    }

    /// Changes a port direction in place. The surrounding circuit can
    /// be temporarily invalid; compilation remains the structural
    /// validator, as with the other editor operations.
    pub fn set_port_direction(&mut self, port: PortId, direction: PortDirection) -> Result<()> {
        let direction = match direction {
            PortDirection::Input => DIRECTION_IN,
            PortDirection::Output => DIRECTION_OUT,
        };
        self.replace_property(
            iri(PORT_PREFIX, port.0),
            PRED_PORT_DIRECTION,
            RdfTerm::literal(direction),
        )
    }

    /// Removes a port and every wire referencing it.
    pub fn remove_port(&mut self, port: PortId) -> Result<()> {
        let port_iri = iri(PORT_PREFIX, port.0);
        self.remove_wires_referencing(&port_iri, &[PRED_WIRE_FROM_PORT, PRED_WIRE_TO_PORT])?;
        self.remove_subject(port_iri)
    }

    pub fn add_cell(&mut self, module: ModuleId, kind: CellKind) -> Result<CellId> {
        let id = CellId::new_random();
        let subject = iri(CELL_PREFIX, id.0);
        self.add_triple(subject.clone(), RDF_TYPE, RdfTerm::iri(TYPE_CELL))?;
        self.add_triple(subject.clone(), PRED_CELL_OF, iri(MODULE_PREFIX, module.0))?;
        match kind {
            CellKind::Nand => {
                self.add_triple(subject, PRED_INSTANCE_OF, RdfTerm::iri(IRI_NAND))?;
            }
            CellKind::Reg { init } => {
                self.add_triple(subject.clone(), PRED_INSTANCE_OF, RdfTerm::iri(IRI_REG))?;
                self.add_triple(
                    subject,
                    PRED_REG_INIT,
                    RdfTerm::literal(if init { "true" } else { "false" }),
                )?;
            }
            CellKind::Instance { module: inner } => {
                self.add_triple(subject, PRED_INSTANCE_OF, iri(MODULE_PREFIX, inner.0))?;
            }
        }
        Ok(id)
    }

    /// Replaces a cell's kind while preserving its identity. Existing
    /// wires are deliberately left untouched; callers can rewire the
    /// cell in the same edit batch and the compiler reports any pin
    /// mismatch in the interim.
    pub fn set_cell_kind(&mut self, cell: CellId, kind: CellKind) -> Result<()> {
        let subject = iri(CELL_PREFIX, cell.0);
        match kind {
            CellKind::Nand => {
                self.replace_property(subject.clone(), PRED_INSTANCE_OF, RdfTerm::iri(IRI_NAND))?;
                self.remove_property(&subject, PRED_REG_INIT)
            }
            CellKind::Reg { init } => {
                self.replace_property(subject.clone(), PRED_INSTANCE_OF, RdfTerm::iri(IRI_REG))?;
                self.replace_property(
                    subject,
                    PRED_REG_INIT,
                    RdfTerm::literal(if init { "true" } else { "false" }),
                )
            }
            CellKind::Instance { module } => {
                self.replace_property(
                    subject.clone(),
                    PRED_INSTANCE_OF,
                    iri(MODULE_PREFIX, module.0),
                )?;
                self.remove_property(&subject, PRED_REG_INIT)
            }
        }
    }

    pub fn set_reg_init(&mut self, cell: CellId, init: bool) -> Result<()> {
        self.replace_property(
            iri(CELL_PREFIX, cell.0),
            PRED_REG_INIT,
            RdfTerm::literal(if init { "true" } else { "false" }),
        )
    }

    /// Removes a cell and every wire referencing it.
    pub fn remove_cell(&mut self, cell: CellId) -> Result<()> {
        let cell_iri = iri(CELL_PREFIX, cell.0);
        self.remove_wires_referencing(&cell_iri, &[PRED_WIRE_FROM_CELL, PRED_WIRE_TO_CELL])?;
        self.remove_subject(cell_iri)
    }

    /// Adds one immutable wire. To change a connection, remove the old
    /// wire and add a new one.
    pub fn add_wire(&mut self, module: ModuleId, from: Endpoint, to: Endpoint) -> Result<WireId> {
        let id = WireId::new_random();
        let subject = iri(WIRE_PREFIX, id.0);
        self.add_triple(subject.clone(), RDF_TYPE, RdfTerm::iri(TYPE_WIRE))?;
        self.add_triple(subject.clone(), PRED_WIRE_OF, iri(MODULE_PREFIX, module.0))?;
        self.write_endpoint(&subject, from, PRED_WIRE_FROM_CELL, PRED_WIRE_FROM_PORT)?;
        self.write_endpoint(&subject, to, PRED_WIRE_TO_CELL, PRED_WIRE_TO_PORT)?;
        Ok(id)
    }

    pub fn remove_wire(&mut self, wire: WireId) -> Result<()> {
        self.remove_subject(iri(WIRE_PREFIX, wire.0))
    }

    // ------------------------------------------------------ helpers

    fn add_triple(&mut self, subject: RdfTerm, predicate: &str, object: RdfTerm) -> Result<()> {
        let operation = self
            .graph
            .add_triple(subject, predicate, object)
            .map_err(|e| CircuitCrdfError::AddTriple(format!("{e:?}")))?;
        self.operations.push(operation);
        Ok(())
    }

    fn remove_triple(
        &mut self,
        subject: &RdfTerm,
        predicate: &str,
        object: &RdfTerm,
    ) -> Result<()> {
        let operation = self
            .graph
            .remove_triple(subject, predicate, object)
            .map_err(|e| CircuitCrdfError::RemoveTriple(format!("{e:?}")))?;
        self.operations.push(operation);
        Ok(())
    }

    fn write_endpoint(
        &mut self,
        wire_subject: &RdfTerm,
        endpoint: Endpoint,
        cell_predicate: &str,
        port_predicate: &str,
    ) -> Result<()> {
        match endpoint {
            Endpoint::Module { port } => self.add_triple(
                wire_subject.clone(),
                port_predicate,
                iri(PORT_PREFIX, port.0),
            ),
            Endpoint::Cell { cell, port } => {
                self.add_triple(
                    wire_subject.clone(),
                    cell_predicate,
                    iri(CELL_PREFIX, cell.0),
                )?;
                let port_term = match port {
                    PortRef::NandA => RdfTerm::iri(IRI_NAND_A),
                    PortRef::NandB => RdfTerm::iri(IRI_NAND_B),
                    PortRef::NandY => RdfTerm::iri(IRI_NAND_Y),
                    PortRef::RegD => RdfTerm::iri(IRI_REG_D),
                    PortRef::RegQ => RdfTerm::iri(IRI_REG_Q),
                    PortRef::Inner(inner) => iri(PORT_PREFIX, inner.0),
                };
                self.add_triple(wire_subject.clone(), port_predicate, port_term)
            }
        }
    }

    /// Removes every triple whose subject is `subject`.
    fn remove_subject(&mut self, subject: RdfTerm) -> Result<()> {
        for triple in self.graph.triples_for_subject(&subject) {
            self.remove_triple(&triple.subject, &triple.predicate, &triple.object)?;
        }
        Ok(())
    }

    /// Removes every wire that references `target` through one of the
    /// given endpoint predicates.
    fn remove_wires_referencing(&mut self, target: &RdfTerm, predicates: &[&str]) -> Result<()> {
        let mut wires: Vec<RdfTerm> = Vec::new();
        for predicate in predicates {
            for wire in self.graph.subjects_for_predicate_object(predicate, target) {
                if !wires.contains(&wire) {
                    wires.push(wire);
                }
            }
        }
        for wire in wires {
            self.remove_subject(wire)?;
        }
        Ok(())
    }

    /// Replaces the value(s) of a functional property.
    fn replace_property(
        &mut self,
        subject: RdfTerm,
        predicate: &str,
        value: RdfTerm,
    ) -> Result<()> {
        for existing in self
            .graph
            .objects_for_subject_predicate(&subject, predicate)
        {
            self.remove_triple(&subject, predicate, &existing)?;
        }
        self.add_triple(subject, predicate, value)
    }

    fn remove_property(&mut self, subject: &RdfTerm, predicate: &str) -> Result<()> {
        for existing in self.graph.objects_for_subject_predicate(subject, predicate) {
            self.remove_triple(subject, predicate, &existing)?;
        }
        Ok(())
    }
}
