//! The public CRDF circuit vocabulary (schema version 1).
//!
//! Publication-grade identifiers:
//!
//! - **Vocabulary** (classes, predicates, axiom boxes and their pins)
//!   lives under the namespace [`CIRCUIT_NS`] =
//!   `https://bkbkb.net/crdf/circuit#`, a domain operated by the
//!   project. Publication checklist: serve the spec (HTML) and a
//!   machine-readable vocabulary (Turtle) at that URL with content
//!   negotiation, and keep the domain registered — these IRIs are
//!   permanent identifiers, so the path must never be repurposed.
//!   Until hosting exists the IRIs are still valid, stable
//!   identifiers.
//! - **Instances** (assets, modules, ports, cells, wires — user data)
//!   use the standard `urn:uuid:` form (RFC 9562). Their kind is
//!   carried by `rdf:type` triples, not by the identifier, following
//!   linked-data practice: identifiers are opaque, meaning lives in
//!   triples (`moduleName`, `portName`, …).
//!
//! External tools should reference these constants (or the documented
//! namespace) instead of duplicating strings. Suggested prefix map for
//! serialisations:
//!
//! ```text
//! PREFIX circuit: <https://bkbkb.net/crdf/circuit#>
//! PREFIX rdf:     <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
//! ```

/// Namespace of the circuit vocabulary.
pub const CIRCUIT_NS: &str = "https://bkbkb.net/crdf/circuit#";

/// The standard RDF `type` predicate.
pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

pub const TYPE_ASSET: &str = "https://bkbkb.net/crdf/circuit#Asset";
pub const TYPE_MODULE: &str = "https://bkbkb.net/crdf/circuit#Module";
pub const TYPE_PORT: &str = "https://bkbkb.net/crdf/circuit#Port";
pub const TYPE_CELL: &str = "https://bkbkb.net/crdf/circuit#Cell";
pub const TYPE_WIRE: &str = "https://bkbkb.net/crdf/circuit#Wire";
/// A typed bus: an annotation grouping several 1-bit ports of a module
/// under one fixed-point [`crate::types::Type`] (schema version 1
/// extension).
pub const TYPE_BUS: &str = "https://bkbkb.net/crdf/circuit#Bus";

pub const PRED_SCHEMA_VERSION: &str = "https://bkbkb.net/crdf/circuit#schemaVersion";
pub const PRED_ROOT_MODULE: &str = "https://bkbkb.net/crdf/circuit#rootModule";
pub const PRED_TICKS_PER_STEP: &str = "https://bkbkb.net/crdf/circuit#ticksPerStep";
pub const PRED_MODULE_NAME: &str = "https://bkbkb.net/crdf/circuit#moduleName";
pub const PRED_PORT_OF: &str = "https://bkbkb.net/crdf/circuit#portOf";
pub const PRED_PORT_NAME: &str = "https://bkbkb.net/crdf/circuit#portName";
pub const PRED_PORT_DIRECTION: &str = "https://bkbkb.net/crdf/circuit#portDirection";
pub const PRED_CELL_OF: &str = "https://bkbkb.net/crdf/circuit#cellOf";
pub const PRED_INSTANCE_OF: &str = "https://bkbkb.net/crdf/circuit#instanceOf";
pub const PRED_REG_INIT: &str = "https://bkbkb.net/crdf/circuit#regInit";
pub const PRED_WIRE_OF: &str = "https://bkbkb.net/crdf/circuit#wireOf";
pub const PRED_WIRE_FROM_CELL: &str = "https://bkbkb.net/crdf/circuit#wireFromCell";
pub const PRED_WIRE_FROM_PORT: &str = "https://bkbkb.net/crdf/circuit#wireFromPort";
pub const PRED_WIRE_TO_CELL: &str = "https://bkbkb.net/crdf/circuit#wireToCell";
pub const PRED_WIRE_TO_PORT: &str = "https://bkbkb.net/crdf/circuit#wireToPort";

/// Typed-bus interface predicates (schema version 1 extension).
pub const PRED_BUS_OF: &str = "https://bkbkb.net/crdf/circuit#busOf";
pub const PRED_BUS_NAME: &str = "https://bkbkb.net/crdf/circuit#busName";
pub const PRED_BUS_DIRECTION: &str = "https://bkbkb.net/crdf/circuit#busDirection";
pub const PRED_BUS_TYPE: &str = "https://bkbkb.net/crdf/circuit#busType";
/// Role tag of a bus (`crate::block_dag::RoleTag`), opaque to the
/// engine. Absent means `raw`.
pub const PRED_BUS_ROLE: &str = "https://bkbkb.net/crdf/circuit#busRole";

/// Block-DAG (high-level typed graph) vocabulary (schema version 1
/// extension).
pub const TYPE_BLOCK_GRAPH: &str = "https://bkbkb.net/crdf/circuit#BlockGraph";
pub const TYPE_BLOCK_INSTANCE: &str = "https://bkbkb.net/crdf/circuit#BlockInstance";
pub const TYPE_CONNECTION: &str = "https://bkbkb.net/crdf/circuit#Connection";
pub const TYPE_EXPOSED: &str = "https://bkbkb.net/crdf/circuit#Exposed";
pub const PRED_MEMBER_OF: &str = "https://bkbkb.net/crdf/circuit#memberOf";
pub const PRED_BLOCK_INDEX: &str = "https://bkbkb.net/crdf/circuit#blockIndex";
pub const PRED_BLOCK_MODULE: &str = "https://bkbkb.net/crdf/circuit#blockModule";
/// Optional editor metadata on a block instance.
pub const PRED_BLOCK_LABEL: &str = "https://bkbkb.net/crdf/circuit#blockLabel";
pub const PRED_BLOCK_POSITION_X: &str = "https://bkbkb.net/crdf/circuit#blockPositionX";
pub const PRED_BLOCK_POSITION_Y: &str = "https://bkbkb.net/crdf/circuit#blockPositionY";
pub const PRED_BLOCK_WIDGET_STYLE: &str = "https://bkbkb.net/crdf/circuit#blockWidgetStyle";
pub const PRED_BLOCK_MANUAL_VALUE: &str = "https://bkbkb.net/crdf/circuit#blockManualValue";
pub const PRED_BLOCK_DEFAULT_VALUE: &str = "https://bkbkb.net/crdf/circuit#blockDefaultValue";
/// A block's control slot (`BlockMetadata::control_slot`). The IRI keeps
/// the name it was first saved under.
pub const PRED_BLOCK_CONTROL_SLOT: &str = "https://bkbkb.net/crdf/circuit#blockAutomationSlot";
/// Whether an editor folds this block into the input it drives instead of
/// drawing it as a separate box. Presentation only.
pub const PRED_BLOCK_DOCKED: &str = "https://bkbkb.net/crdf/circuit#blockDocked";
/// How an editor lays a block out (`pins` / `panel`). Presentation only.
pub const PRED_BLOCK_VIEW: &str = "https://bkbkb.net/crdf/circuit#blockView";
/// Editor metadata for the graph as a whole: where each boundary node
/// (an editor's box for an external input or output) sits.
pub const TYPE_BOUNDARY_NODE: &str = "https://bkbkb.net/crdf/circuit#BoundaryNode";
pub const PRED_BOUNDARY_NAME: &str = "https://bkbkb.net/crdf/circuit#boundaryName";
pub const PRED_BOUNDARY_POSITION_X: &str = "https://bkbkb.net/crdf/circuit#boundaryPositionX";
pub const PRED_BOUNDARY_POSITION_Y: &str = "https://bkbkb.net/crdf/circuit#boundaryPositionY";
/// A boundary node's place among the others, so a save of what was read
/// keeps the order it was authored in (2026-09-27).
pub const PRED_BOUNDARY_INDEX: &str = "https://bkbkb.net/crdf/circuit#boundaryIndex";
pub const PRED_CONN_FROM_BLOCK: &str = "https://bkbkb.net/crdf/circuit#connFromBlock";
pub const PRED_CONN_FROM_BUS: &str = "https://bkbkb.net/crdf/circuit#connFromBus";
pub const PRED_CONN_TO_BLOCK: &str = "https://bkbkb.net/crdf/circuit#connToBlock";
pub const PRED_CONN_TO_BUS: &str = "https://bkbkb.net/crdf/circuit#connToBus";
pub const PRED_EXPOSED_NAME: &str = "https://bkbkb.net/crdf/circuit#exposedName";
pub const PRED_EXPOSED_BLOCK: &str = "https://bkbkb.net/crdf/circuit#exposedBlock";
pub const PRED_EXPOSED_BUS: &str = "https://bkbkb.net/crdf/circuit#exposedBus";
pub const PRED_EXPOSED_TYPE: &str = "https://bkbkb.net/crdf/circuit#exposedType";
pub const PRED_EXPOSED_DIRECTION: &str = "https://bkbkb.net/crdf/circuit#exposedDirection";
/// An exposed port's place among the others, for the same reason.
pub const PRED_EXPOSED_INDEX: &str = "https://bkbkb.net/crdf/circuit#exposedIndex";

/// A block input driven by a literal constant value instead of a wire
/// (`constValue` is the decimal LSB-0 bit pattern over the bus).
pub const TYPE_CONSTANT: &str = "https://bkbkb.net/crdf/circuit#Constant";
pub const PRED_CONST_BLOCK: &str = "https://bkbkb.net/crdf/circuit#constBlock";
pub const PRED_CONST_BUS: &str = "https://bkbkb.net/crdf/circuit#constBus";
pub const PRED_CONST_VALUE: &str = "https://bkbkb.net/crdf/circuit#constValue";

/// A named binding of a block input (`BlockDag::add_binding`): a caller's
/// own name for an input it drives from outside the graph. The tag's IRI
/// keeps the name it was first saved under.
pub const TYPE_BINDING: &str = "https://bkbkb.net/crdf/circuit#Binding";
pub const PRED_BIND_TAG: &str = "https://bkbkb.net/crdf/circuit#bindFrame";
pub const PRED_BIND_BLOCK: &str = "https://bkbkb.net/crdf/circuit#bindBlock";
pub const PRED_BIND_BUS: &str = "https://bkbkb.net/crdf/circuit#bindBus";

/// Instance identifiers are plain RFC 9562 UUID URNs; the entity kind
/// is expressed by `rdf:type`, so every kind shares this prefix.
pub const ENTITY_PREFIX: &str = "urn:uuid:";
pub const ASSET_PREFIX: &str = ENTITY_PREFIX;
pub const MODULE_PREFIX: &str = ENTITY_PREFIX;
pub const PORT_PREFIX: &str = ENTITY_PREFIX;
pub const CELL_PREFIX: &str = ENTITY_PREFIX;
pub const WIRE_PREFIX: &str = ENTITY_PREFIX;

/// The two axiom boxes and their pins.
pub const IRI_NAND: &str = "https://bkbkb.net/crdf/circuit#nand";
pub const IRI_REG: &str = "https://bkbkb.net/crdf/circuit#reg";
pub const IRI_NAND_A: &str = "https://bkbkb.net/crdf/circuit#nand-a";
pub const IRI_NAND_B: &str = "https://bkbkb.net/crdf/circuit#nand-b";
pub const IRI_NAND_Y: &str = "https://bkbkb.net/crdf/circuit#nand-y";
pub const IRI_REG_D: &str = "https://bkbkb.net/crdf/circuit#reg-d";
pub const IRI_REG_Q: &str = "https://bkbkb.net/crdf/circuit#reg-q";

pub const DIRECTION_IN: &str = "in";
pub const DIRECTION_OUT: &str = "out";

// ---- Structural provenance (`crate::structure`) -----------------------
//
// How a module was built, when its author said so. Read by viewers and
// ignored by every evaluator: a document without these is *undescribed*,
// never different.

pub const TYPE_STRUCTURE: &str = "https://bkbkb.net/crdf/circuit#Structure";
pub const PRED_STRUCTURE_OF: &str = "https://bkbkb.net/crdf/circuit#structureOf";
/// `chain` (a scan) or `fold` (a tree).
pub const PRED_STRUCTURE_KIND: &str = "https://bkbkb.net/crdf/circuit#structureKind";
pub const PRED_STRUCTURE_LABEL: &str = "https://bkbkb.net/crdf/circuit#structureLabel";
/// The module every member instantiates, when the body **is** a module.
///
/// Absent for a repetition over primitives, which has no module to name.
/// A reader that finds this and no [`PRED_STRUCTURE_BODY_KIND`] is
/// looking at a file written before a bank of registers could be
/// described, and an instance body is the only thing such a file can
/// mean.
pub const PRED_STRUCTURE_BODY: &str = "https://bkbkb.net/crdf/circuit#structureBody";
/// What kind of cell every member is: `instance`, `nand`, `reg0`, `reg1`.
///
/// Written always, alongside the module for the instance case. A
/// register bank is as real a repetition as sixteen instances of a
/// module, and saying "instance" by omission would have made the two
/// indistinguishable on the way back in.
pub const PRED_STRUCTURE_BODY_KIND: &str = "https://bkbkb.net/crdf/circuit#structureBodyKind";
/// The member cells in elaboration order: uuids separated by spaces,
/// levels of a fold separated by `|`. One literal rather than an ordered
/// collection, for the same reason a wide pinned value is one literal —
/// order matters and RDF has no cheap ordered list.
pub const PRED_STRUCTURE_CELLS: &str = "https://bkbkb.net/crdf/circuit#structureCells";
/// The term's place in its module's list, so a save restores the order
/// it was written in.
///
/// Each structure gets a fresh random subject, and reading walks subjects
/// in sorted order -- so without this the terms come back shuffled. It
/// went unnoticed while no module had more than one; modules with
/// several retained terms need their original order preserved.
pub const PRED_STRUCTURE_INDEX: &str = "https://bkbkb.net/crdf/circuit#structureIndex";

/// A module's claim that a backend may substitute for its gates
/// (`model::IntrinsicDecl`). Unlike a `Structure`, this one is
/// load-bearing, so it has to survive a save: a declaration that
/// vanishes on reload is a circuit that quietly gets slower, and worse,
/// one whose speed depends on how it was loaded.
pub const TYPE_INTRINSIC: &str = "https://bkbkb.net/crdf/circuit#Intrinsic";
pub const PRED_INTRINSIC_OF: &str = "https://bkbkb.net/crdf/circuit#intrinsicOf";
pub const PRED_INTRINSIC_NAME: &str = "https://bkbkb.net/crdf/circuit#intrinsicName";
pub const PRED_INTRINSIC_REVISION: &str = "https://bkbkb.net/crdf/circuit#intrinsicRevision";
pub const PRED_INTRINSIC_INPUTS: &str = "https://bkbkb.net/crdf/circuit#intrinsicInputs";
pub const PRED_INTRINSIC_OUTPUTS: &str = "https://bkbkb.net/crdf/circuit#intrinsicOutputs";
