//! The block-DAG → single-asset compiler.
//!
//! A [`BlockDag`] is a high layer over the netlist: reusable library
//! modules (adders, multipliers, state machines, …) placed as typed
//! **blocks** and wired with typed **buses**. [`BlockDag::compile`] lowers
//! it to a *single* [`CircuitAsset`] — one module that instantiates every
//! block, wires each connection, and, wherever two ports' fixed-point
//! types differ, **auto-synthesises the glue** via
//! [`plan_connection`] + [`build_word_convert`]. Runtime is
//! unchanged (one asset, the usual bit-sliced/JIT evaluators); only
//! authoring is easy.
//!
//! This is a minimal but real compiler: it checks the single-driver rule
//! (fan-out is free, merging is not) and reports type mismatches. It does
//! *not* yet do the editor-side `combDeps` cycle pre-check (phase ⑤) —
//! the existing `flatten` zero-delay-cycle check remains the ground truth
//! for feedback safety.

use crate::adapter::build_word_convert;
use crate::connect::{Mismatch, WordFit, plan_connection};
use crate::model::{CellId, CircuitAsset, Endpoint, ModuleBuilder, ModuleId, ModuleLibrary};
use crate::stdlib::{Stdlib, try_port_id};
use crate::types::{Type, Word};
use std::borrow::Cow;
use uuid::Uuid;

/// A typed bus's **role**: what its signal means beyond its bit
/// representation, as a tag the engine keeps and persists but does not
/// interpret. A caller gives roles their meaning and decides which may be
/// connected; to the engine two roles are only equal or not. Roles live on
/// the block interface, not on [`Type`]: types describe representation,
/// not meaning. [`RoleTag::RAW`] marks a bus that claims no role.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RoleTag(Cow<'static, str>);

impl RoleTag {
    /// A bus that claims no role.
    pub const RAW: RoleTag = RoleTag(Cow::Borrowed("raw"));

    /// A role named by a static tag.
    pub const fn from_static(tag: &'static str) -> Self {
        RoleTag(Cow::Borrowed(tag))
    }

    /// A role named by `tag`, kept exactly as given.
    pub fn new(tag: impl Into<String>) -> Self {
        RoleTag(Cow::Owned(tag.into()))
    }

    /// The tag, as it is persisted.
    pub fn as_tag(&self) -> &str {
        &self.0
    }
}

/// The typed interface of a library module used as a block: its id and the
/// `(bus name, type, role)` of each input and output bus. A bus's
/// underlying 1-bit ports are the type's [`Type::leaf_names`] under the bus
/// name, so the interface must match the module's actual port names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockInterface {
    pub module: ModuleId,
    pub inputs: Vec<(String, Type, RoleTag)>,
    pub outputs: Vec<(String, Type, RoleTag)>,
}

impl BlockInterface {
    fn input_type(&self, bus: &str) -> Option<&Type> {
        self.inputs
            .iter()
            .find(|(n, _, _)| n == bus)
            .map(|(_, t, _)| t)
    }
    fn output_type(&self, bus: &str) -> Option<&Type> {
        self.outputs
            .iter()
            .find(|(n, _, _)| n == bus)
            .map(|(_, t, _)| t)
    }

    /// The role of an input bus, if it exists.
    pub fn input_role(&self, bus: &str) -> Option<RoleTag> {
        self.inputs
            .iter()
            .find(|(n, _, _)| n == bus)
            .map(|(_, _, r)| r.clone())
    }
    /// The role of an output bus, if it exists.
    pub fn output_role(&self, bus: &str) -> Option<RoleTag> {
        self.outputs
            .iter()
            .find(|(n, _, _)| n == bus)
            .map(|(_, _, r)| r.clone())
    }
}

/// A reference to one bus on one placed block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusRef {
    pub block: usize,
    pub bus: String,
}

/// Editor metadata attached to a placed block.
///
/// None of these fields changes the compiled circuit. They are persisted
/// alongside the editable block graph so a patch keeps its identity and
/// presentation across save/load. Older CRDF documents simply omit the
/// optional fields and receive safe defaults when loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockMetadata {
    /// Stable identity of this particular placement, independent of its
    /// transient index in [`BlockDag::blocks`].
    pub id: Uuid,
    /// User-visible label. `None` asks the editor to derive one from the
    /// module name.
    pub label: Option<String>,
    /// Canvas position in patch/world coordinates.
    pub position: Option<[f32; 2]>,
    /// Presentation tag such as `knob`, `h-slider`, or `number`.
    pub widget_style: Option<String>,
    /// Manual value retained while an external cable drives a control
    /// widget, and restored when that cable is removed.
    pub manual_value: Option<u64>,
    /// Explicit reset value for the control widget. Kept separate from
    /// the current manual value so a double-click never guesses a
    /// semantic default from the signal type.
    pub default_value: Option<u64>,
    /// Optional fixed control slot: an index an embedding application may
    /// give this block, for example to bind one of its own controls. The
    /// engine only stores it; it does not alter the NAND module. Persisted
    /// under `blockAutomationSlot`, its original name.
    pub control_slot: Option<u8>,
    /// How an editor lays this block out: `pins` for a column of jacks,
    /// `panel` for a front panel of its folded controls. Unknown values
    /// fall back to `pins`.
    pub view: Option<String>,
    /// Whether an editor draws this block folded into the input its
    /// output drives, rather than as a box of its own with a cable.
    ///
    /// Presentation only, like [`Self::widget_style`]: the block, its
    /// connection and its value are unchanged, so a docked and an
    /// undocked graph lower to the same circuit.
    pub docked: bool,
}

impl Default for BlockMetadata {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            label: None,
            position: None,
            widget_style: None,
            manual_value: None,
            default_value: None,
            control_slot: None,
            view: None,
            docked: false,
        }
    }
}

/// A high-level typed block graph that compiles to one circuit asset.
#[derive(Debug, Clone)]
pub struct BlockDag {
    /// The graph's own identity, as its blocks have theirs: random when
    /// the graph is made, written as the graph node's IRI and read back.
    /// Everything else a save names — boundary nodes, connections,
    /// exposed ports, constants, bindings — is derived from it and from
    /// what that thing is, so **saving an unchanged graph twice gives the
    /// same facts** (`crdf_io::write_block_dag_into`). An undo history
    /// that diffs two saves finds only what changed.
    id: Uuid,
    blocks: Vec<BlockInterface>,
    block_metadata: Vec<BlockMetadata>,
    connections: Vec<(BusRef, BusRef)>,
    exposed_inputs: Vec<(String, BusRef, Type)>,
    /// Inputs exposed under **explicit** leaf names (not derived from an
    /// external name), for an embedder whose port names do not follow the
    /// default LSB-0 scheme — e.g. a 24-bit input exposed as bits
    /// `x.8..31` of a wider external word. These are transient (used when
    /// lowering for such an embedder) and are *not* persisted; the
    /// persistable, name-based exposures are `exposed_inputs`.
    exposed_inputs_named: Vec<(Vec<String>, BusRef, Type)>,
    exposed_outputs: Vec<(String, BusRef, Type)>,
    /// Input buses driven by a **literal constant** rather than a wire —
    /// a constant in its cheapest form: a fixed value needs no module, no
    /// boundary port and no binding, just the two axiom-box constants the
    /// compiler already has.
    /// The value's bits are LSB-0 over the bus's lowering order.
    constants: Vec<(BusRef, Vec<u64>)>,
    /// Named bindings carried for **persistence only** — `(tag, target
    /// input)`: a caller's own name for an input it drives from outside the
    /// graph. The compiler ignores them; they exist so a saved graph
    /// round-trips that information.
    bindings: Vec<(String, BusRef)>,
    /// Editor metadata for the graph as a whole: where the boundary nodes
    /// an editor draws around the patch sit, in the same coordinates as
    /// [`BlockMetadata::position`]. Carried here so that an arrangement
    /// round-trips completely rather than only for the blocks.
    boundary_positions: Vec<(String, [f32; 2])>,
}

/// Why a [`BlockDag`] does not compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockDagError {
    /// A `BusRef` names a block index that does not exist.
    UnknownBlock(usize),
    /// A block has no such input/output bus.
    UnknownBus { block: usize, bus: String },
    /// An input bus is driven by more than one source (single-driver rule).
    MultipleDrivers { block: usize, bus: String },
    /// An input bus is left undriven.
    UndrivenInput { block: usize, bus: String },
    /// A connection's two ports have incompatible shapes.
    TypeMismatch {
        from: BusRef,
        to: BusRef,
        reason: Mismatch,
    },
    /// [`BlockDag::interface`] was asked for a patch that exposes a port
    /// under explicit leaf names. Those follow an embedder's port naming,
    /// not a bus's, so there is no bus name to present them under.
    NotABus { leaf_names: Vec<String> },
    /// Two exposures claim the same boundary signal while disagreeing
    /// about what it is. See `BlockDag::validate_boundary`.
    BoundaryNameClash { name: String },
    /// A block's declared interface names a leaf port its module does not
    /// have, or names a module that is not in the library.
    ///
    /// Unreachable for an interface written in Rust beside its module,
    /// which is every one this crate ships. It becomes reachable the
    /// moment an interface can be restored from a file, and it used to be
    /// a panic — `port_id` says of itself that it panics when a port is
    /// absent, and the compiler called it on names it had not checked.
    NoSuchPort { bus: String, port: String },
}

impl std::fmt::Display for BlockDagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockDagError::UnknownBlock(i) => write!(f, "unknown block index {i}"),
            BlockDagError::UnknownBus { block, bus } => {
                write!(f, "block {block} has no bus {bus:?}")
            }
            BlockDagError::MultipleDrivers { block, bus } => {
                write!(f, "input {bus:?} of block {block} has multiple drivers")
            }
            BlockDagError::UndrivenInput { block, bus } => {
                write!(f, "input {bus:?} of block {block} is undriven")
            }
            BlockDagError::TypeMismatch { from, to, reason } => write!(
                f,
                "cannot connect {}.{} to {}.{}: {reason}",
                from.block, from.bus, to.block, to.bus
            ),
            BlockDagError::NotABus { leaf_names } => write!(
                f,
                "this patch exposes {leaf_names:?} as explicitly named ports, not as a \
                 bus, so it cannot be placed inside another patch"
            ),
            BlockDagError::BoundaryNameClash { name } => write!(
                f,
                "the boundary signal {name:?} is claimed twice, as two different things"
            ),
            BlockDagError::NoSuchPort { bus, port } => write!(
                f,
                "bus {bus:?} names the port {port:?}, which its module does not have"
            ),
        }
    }
}

impl std::error::Error for BlockDagError {}

impl Default for BlockDag {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            blocks: Vec::new(),
            block_metadata: Vec::new(),
            connections: Vec::new(),
            exposed_inputs: Vec::new(),
            exposed_inputs_named: Vec::new(),
            exposed_outputs: Vec::new(),
            constants: Vec::new(),
            bindings: Vec::new(),
            boundary_positions: Vec::new(),
        }
    }
}

impl BlockDag {
    pub fn new() -> Self {
        Self::default()
    }

    /// The graph's persistent identity (see the field).
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Adopts an identity — the one an editable document keeps across the
    /// graphs it builds, or the one a save carried.
    pub fn set_id(&mut self, id: Uuid) {
        self.id = id;
    }

    /// Places a block; returns its index for use in [`BusRef`]s.
    pub fn add_block(&mut self, interface: BlockInterface) -> usize {
        self.add_block_with_metadata(interface, BlockMetadata::default())
    }

    /// Places a block with editor metadata; returns its transient index.
    pub fn add_block_with_metadata(
        &mut self,
        interface: BlockInterface,
        metadata: BlockMetadata,
    ) -> usize {
        self.blocks.push(interface);
        self.block_metadata.push(metadata);
        self.blocks.len() - 1
    }

    /// The placed blocks, indexed by [`BusRef::block`].
    pub fn blocks(&self) -> &[BlockInterface] {
        &self.blocks
    }

    /// Records where an editor's boundary node sits. `name` is the
    /// editor's own stable key for that node; this crate does not
    /// interpret it.
    pub fn set_boundary_position(&mut self, name: impl Into<String>, pos: [f32; 2]) {
        let name = name.into();
        match self
            .boundary_positions
            .iter_mut()
            .find(|(existing, _)| *existing == name)
        {
            Some((_, slot)) => *slot = pos,
            None => self.boundary_positions.push((name, pos)),
        }
    }

    /// The recorded boundary-node positions.
    pub fn boundary_positions(&self) -> &[(String, [f32; 2])] {
        &self.boundary_positions
    }

    /// Editor metadata parallel to [`Self::blocks`].
    pub fn block_metadata(&self) -> &[BlockMetadata] {
        &self.block_metadata
    }

    /// The connections, as `(output bus, input bus)` pairs.
    pub fn connections(&self) -> &[(BusRef, BusRef)] {
        &self.connections
    }

    /// The exposed external inputs: `(external name, target input bus, type)`.
    pub fn exposed_inputs(&self) -> &[(String, BusRef, Type)] {
        &self.exposed_inputs
    }

    /// The exposed external outputs: `(external name, source output bus, type)`.
    pub fn exposed_outputs(&self) -> &[(String, BusRef, Type)] {
        &self.exposed_outputs
    }

    /// Records a named binding of a block input for persistence
    /// (`tag` → `(block, bus)`). Does not affect [`Self::compile`].
    pub fn add_binding(&mut self, tag: &str, block: usize, bus: &str) {
        self.bindings.push((
            tag.to_string(),
            BusRef {
                block,
                bus: bus.to_string(),
            },
        ));
    }

    /// The recorded bindings: `(tag, target input)`.
    pub fn bindings(&self) -> &[(String, BusRef)] {
        &self.bindings
    }

    /// Connects output bus `from_bus` of block `from` to input bus `to_bus`
    /// of block `to`.
    pub fn connect(&mut self, from: usize, from_bus: &str, to: usize, to_bus: &str) {
        self.connections.push((
            BusRef {
                block: from,
                bus: from_bus.to_string(),
            },
            BusRef {
                block: to,
                bus: to_bus.to_string(),
            },
        ));
    }

    /// Drives input bus `(block, bus)` from a **constant** `value` instead
    /// of a wire: bit `i` of `value` (LSB-0, in the bus's lowering order)
    /// becomes a fixed 0 or 1. This counts as the bus's single driver, so a
    /// fixed value costs neither an exposed input nor a binding. Bus bits
    /// beyond 64 are zero.
    pub fn set_constant(&mut self, block: usize, bus: &str, value: u64) {
        self.set_constant_words(block, bus, &[value]);
    }

    /// Pins a bus **wider than a word**, least significant word first.
    ///
    /// A bus is as wide as its type, and a type may be an aggregate: a
    /// curve of sixty-four sixteen-bit points is one bus of 1,024 bits.
    /// A single `u64` silently zeroed everything above bit 63, which was
    /// unreachable while every bus was a word and became a real way to
    /// lose data the moment one was not.
    ///
    /// Bits past the end of `words` are zero, which is what pinning a
    /// short value into a wide bus should mean.
    pub fn set_constant_words(&mut self, block: usize, bus: &str, words: &[u64]) {
        self.constants.push((
            BusRef {
                block,
                bus: bus.to_string(),
            },
            words.to_vec(),
        ));
    }

    /// The constant-driven inputs: `(target input bus, value words,
    /// least significant first)`.
    pub fn constants(&self) -> &[(BusRef, Vec<u64>)] {
        &self.constants
    }

    /// Exposes block input bus `(block, bus)` as an external module input
    /// named `ext_name` of type `ty`.
    pub fn expose_input(&mut self, ext_name: &str, block: usize, bus: &str, ty: Type) {
        self.exposed_inputs.push((
            ext_name.to_string(),
            BusRef {
                block,
                bus: bus.to_string(),
            },
            ty,
        ));
    }

    /// Exposes block output bus `(block, bus)` as an external module output
    /// named `ext_name` of type `ty`.
    pub fn expose_output(&mut self, ext_name: &str, block: usize, bus: &str, ty: Type) {
        self.exposed_outputs.push((
            ext_name.to_string(),
            BusRef {
                block,
                bus: bus.to_string(),
            },
            ty,
        ));
    }

    /// Exposes a block input under **explicit** module-input leaf names
    /// (LSB-0 order), rather than deriving them from an external name. Use
    /// this for ports with offset naming, e.g. a 24-bit input exposed as
    /// bits `x.8..=31` of a wider external word.
    ///
    /// This is the general form: [`Self::expose_input`] is the same thing
    /// with the leaf names derived from a base name
    /// ([`Type::leaf_names`]), and differs only in also being *persisted*
    /// under that name.
    pub fn expose_input_named(
        &mut self,
        leaf_names: Vec<String>,
        block: usize,
        bus: &str,
        ty: Type,
    ) {
        self.exposed_inputs_named.push((
            leaf_names,
            BusRef {
                block,
                bus: bus.to_string(),
            },
            ty,
        ));
    }

    /// The [`BlockInterface`] this patch presents when it is placed
    /// **inside another patch** — the first half of patch-as-block.
    ///
    /// There is nothing to invent. [`Self::compile`] already builds a
    /// module whose ports are exactly `ty.leaf_names(ext_name)` for each
    /// exposure, so a patch's boundary is already its interface: name and
    /// type come from the exposure, and the role from the bus it targets.
    /// `module` is the id [`Self::compile`] returned.
    ///
    /// Exposures made under explicit leaf names
    /// ([`Self::expose_input_named`]) follow an embedder's port naming —
    /// `x.8..=31` and the like. They are signals, not buses, and a patch
    /// carrying one is a top-level circuit rather than a component, so this
    /// refuses rather than inventing a name for it.
    pub fn interface(&self, module: ModuleId) -> Result<BlockInterface, BlockDagError> {
        self.validate_boundary()?;
        if let Some((leaf_names, _, _)) = self.exposed_inputs_named.first() {
            return Err(BlockDagError::NotABus {
                leaf_names: leaf_names.clone(),
            });
        }
        let port =
            |ext_name: &String, ty: &Type, role: RoleTag| (ext_name.clone(), ty.clone(), role);
        let inputs = self
            .exposed_inputs
            .iter()
            .map(|(ext_name, target, ty)| {
                let role = self
                    .block(target.block)?
                    .input_role(&target.bus)
                    .ok_or_else(|| BlockDagError::UnknownBus {
                        block: target.block,
                        bus: target.bus.clone(),
                    })?;
                Ok(port(ext_name, ty, role))
            })
            .collect::<Result<Vec<_>, BlockDagError>>()?;
        let outputs = self
            .exposed_outputs
            .iter()
            .map(|(ext_name, source, ty)| {
                let role = self
                    .block(source.block)?
                    .output_role(&source.bus)
                    .ok_or_else(|| BlockDagError::UnknownBus {
                        block: source.block,
                        bus: source.bus.clone(),
                    })?;
                Ok(port(ext_name, ty, role))
            })
            .collect::<Result<Vec<_>, BlockDagError>>()?;
        Ok(BlockInterface {
            module,
            inputs,
            outputs,
        })
    }

    /// Lowers the DAG to a single [`CircuitAsset`]. `library` must contain
    /// every block module (and its transitive dependencies); `stdlib`
    /// supplies the generic gate modules the glue needs.
    pub fn compile(
        &self,
        stdlib: &Stdlib,
        library: &ModuleLibrary,
        ticks_per_step: u8,
    ) -> Result<CircuitAsset, BlockDagError> {
        self.compile_as(ModuleId::new_random(), stdlib, library, ticks_per_step)
    }

    /// [`Self::compile`], but the resulting module carries the identity
    /// **the caller already decided on** instead of a fresh random one.
    ///
    /// This is what makes a patch referenceable. A block records the
    /// module it places, so a definition whose id changed on every
    /// compile could be placed but never saved: reloading would look for
    /// an id that no longer existed. Compiling is a *rendering* of a
    /// definition, and a rendering does not get to name it.
    ///
    /// The id must be minted once, randomly, and kept — **not** derived
    /// from the definition's name. Names collide, and two people's
    /// different work under one name would silently merge into one
    /// definition. Name-derived ids
    /// stay reserved for the built-in library, where the name really is
    /// the identity.
    pub fn compile_as(
        &self,
        id: ModuleId,
        stdlib: &Stdlib,
        library: &ModuleLibrary,
        ticks_per_step: u8,
    ) -> Result<CircuitAsset, BlockDagError> {
        self.validate_boundary()?;
        self.validate_drivers()?;

        let mut m = ModuleBuilder::new("block_dag.root");
        let zero = Endpoint::reg_q(m.constant(false));
        let one = Endpoint::reg_q(m.constant(true));

        // Instantiate every block.
        let cells: Vec<CellId> = self.blocks.iter().map(|b| m.instance(b.module)).collect();

        // Boundary input ports are created once per leaf name and reused:
        // exposing the *same* signal onto several block inputs (one enable
        // driving two blocks, say) is fan-out, which is free — not a
        // duplicate port.
        let mut boundary_ports: Vec<(String, crate::model::PortId)> = Vec::new();

        // Exposed inputs → module input ports → drive the target bus.
        // The external name is just a way of *deriving* the leaf names, so
        // this goes through the same path as an explicitly-named exposure.
        for (ext_name, target, ty) in &self.exposed_inputs {
            let leaf_names = ty.leaf_names(ext_name);
            let src = boundary_input_named(&mut m, &mut boundary_ports, &leaf_names, ty);
            let dst_ty = self.require_input(target)?.clone();
            let dst = self.instance_input_bus(library, &cells, target)?;
            wire_bus(&mut m, stdlib, ty, &src, &dst_ty, &dst, zero, one).map_err(|reason| {
                BlockDagError::TypeMismatch {
                    from: BusRef {
                        block: usize::MAX,
                        bus: ext_name.clone(),
                    },
                    to: target.clone(),
                    reason,
                }
            })?;
        }

        // Inputs exposed under explicit leaf names (an embedder's ports).
        for (leaf_names, target, ty) in &self.exposed_inputs_named {
            let src = boundary_input_named(&mut m, &mut boundary_ports, leaf_names, ty);
            let dst_ty = self.require_input(target)?.clone();
            let dst = self.instance_input_bus(library, &cells, target)?;
            wire_bus(&mut m, stdlib, ty, &src, &dst_ty, &dst, zero, one).map_err(|reason| {
                BlockDagError::TypeMismatch {
                    from: BusRef {
                        block: usize::MAX,
                        bus: leaf_names.first().cloned().unwrap_or_default(),
                    },
                    to: target.clone(),
                    reason,
                }
            })?;
        }

        // Literal constants → drive block inputs from the axiom constants.
        for (target, words) in &self.constants {
            let dst = self.instance_input_bus(library, &cells, target)?;
            for (bit, sink) in dst.iter().flat_map(|(_, eps)| eps).enumerate() {
                let high = words
                    .get(bit / 64)
                    .is_some_and(|word| (word >> (bit % 64)) & 1 == 1);
                m.wire(if high { one } else { zero }, *sink);
            }
        }

        // Connections → drive block inputs from block outputs.
        for (from, to) in &self.connections {
            let src_ty = self.require_output(from)?.clone();
            let dst_ty = self.require_input(to)?.clone();
            let src = self.instance_output_bus(library, &cells, from)?;
            let dst = self.instance_input_bus(library, &cells, to)?;
            wire_bus(&mut m, stdlib, &src_ty, &src, &dst_ty, &dst, zero, one).map_err(
                |reason| BlockDagError::TypeMismatch {
                    from: from.clone(),
                    to: to.clone(),
                    reason,
                },
            )?;
        }

        // Source block outputs → module output ports.
        for (ext_name, source, ty) in &self.exposed_outputs {
            let src_ty = self.require_output(source)?.clone();
            let src = self.instance_output_bus(library, &cells, source)?;
            let dst = self.boundary_output_bus(&mut m, ext_name, ty);
            wire_bus(&mut m, stdlib, &src_ty, &src, ty, &dst, zero, one).map_err(|reason| {
                BlockDagError::TypeMismatch {
                    from: source.clone(),
                    to: BusRef {
                        block: usize::MAX,
                        bus: ext_name.clone(),
                    },
                    reason,
                }
            })?;
        }

        let mut root = m.finish();
        drop_unused_constants(&mut root, &[zero, one]);
        root.id = id;
        let root_id = id;
        let mut merged = library.clone();
        for module in stdlib.library.modules() {
            merged.insert(module.clone());
        }
        merged.insert(root);
        Ok(CircuitAsset::new(root_id, ticks_per_step, merged))
    }

    /// **A boundary name denotes one signal.**
    ///
    /// Two exposures may share a name only if they *are* the same signal
    /// — same leaf names, same type, same role. That is fan-out (one
    /// enable driving two blocks), it costs nothing, and it stays legal. Any
    /// other overlap is two exposures claiming one wire while disagreeing
    /// about what it carries, and the boundary is addressed by leaf name,
    /// so there is no way to tell them apart later.
    ///
    /// Three things this catches, all of which compiled silently before:
    ///
    /// - two inputs sharing a name with **different types** — the second
    ///   quietly reused the first's ports, so a 15-bit coefficient ended
    ///   up driven by the low bits of a 16-bit signed input;
    /// - two outputs sharing a name — two ports, one reachable;
    /// - an input and an output sharing a name. At the top level this is
    ///   two separate port lists and nobody notices, but a nested
    ///   instance resolves a bus by name without regard to direction, so
    ///   the output silently resolves to the input port. Making a patch
    ///   placeable is what turned this from latent into reachable.
    fn validate_boundary(&self) -> Result<(), BlockDagError> {
        /// What a boundary leaf name is claimed to mean. Two exposures may
        /// land on one leaf name only if they claim the very same thing.
        #[derive(Clone, PartialEq)]
        struct Claim {
            leaves: Vec<String>,
            ty: Type,
            role: Option<RoleTag>,
            /// Fan-out is an input-side idea: two outputs of one name are
            /// two ports, however alike.
            shareable: bool,
        }

        let mut claimed: Vec<(String, Claim)> = Vec::new();
        let mut claim = |claim: Claim| -> Result<(), BlockDagError> {
            for leaf in claim.leaves.clone() {
                match claimed.iter().find(|(name, _)| *name == leaf) {
                    Some((_, held)) if claim.shareable && *held == claim => {}
                    Some(_) => return Err(BlockDagError::BoundaryNameClash { name: leaf }),
                    None => claimed.push((leaf, claim.clone())),
                }
            }
            Ok(())
        };

        for (ext_name, target, ty) in &self.exposed_inputs {
            claim(Claim {
                leaves: ty.leaf_names(ext_name),
                ty: ty.clone(),
                role: self.block(target.block)?.input_role(&target.bus),
                shareable: true,
            })?;
        }
        for (leaf_names, target, ty) in &self.exposed_inputs_named {
            claim(Claim {
                leaves: leaf_names.clone(),
                ty: ty.clone(),
                role: self.block(target.block)?.input_role(&target.bus),
                shareable: true,
            })?;
        }
        for (ext_name, source, ty) in &self.exposed_outputs {
            claim(Claim {
                leaves: ty.leaf_names(ext_name),
                ty: ty.clone(),
                role: self.block(source.block)?.output_role(&source.bus),
                shareable: false,
            })?;
        }
        Ok(())
    }

    /// Checks that each block input bus is driven exactly once.
    fn validate_drivers(&self) -> Result<(), BlockDagError> {
        // Count drivers per (block, input-bus).
        let mut driven: Vec<(usize, String)> = Vec::new();
        let mut record = |bus: &BusRef| -> Result<(), BlockDagError> {
            if driven.iter().any(|(b, n)| *b == bus.block && *n == bus.bus) {
                return Err(BlockDagError::MultipleDrivers {
                    block: bus.block,
                    bus: bus.bus.clone(),
                });
            }
            driven.push((bus.block, bus.bus.clone()));
            Ok(())
        };
        for (_, to) in &self.connections {
            self.require_input(to)?;
            record(to)?;
        }
        for (_, target, _) in &self.exposed_inputs {
            self.require_input(target)?;
            record(target)?;
        }
        for (_, target, _) in &self.exposed_inputs_named {
            self.require_input(target)?;
            record(target)?;
        }
        for (target, _) in &self.constants {
            self.require_input(target)?;
            record(target)?;
        }
        // Every declared block input must be driven.
        for (bi, block) in self.blocks.iter().enumerate() {
            for (bus, _, _) in &block.inputs {
                if !driven.iter().any(|(b, n)| *b == bi && n == bus) {
                    return Err(BlockDagError::UndrivenInput {
                        block: bi,
                        bus: bus.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    fn block(&self, index: usize) -> Result<&BlockInterface, BlockDagError> {
        self.blocks
            .get(index)
            .ok_or(BlockDagError::UnknownBlock(index))
    }

    fn require_input(&self, bus: &BusRef) -> Result<&Type, BlockDagError> {
        self.block(bus.block)?
            .input_type(&bus.bus)
            .ok_or_else(|| BlockDagError::UnknownBus {
                block: bus.block,
                bus: bus.bus.clone(),
            })
    }

    fn require_output(&self, bus: &BusRef) -> Result<&Type, BlockDagError> {
        self.block(bus.block)?
            .output_type(&bus.bus)
            .ok_or_else(|| BlockDagError::UnknownBus {
                block: bus.block,
                bus: bus.bus.clone(),
            })
    }

    /// The per-word-leaf endpoints of a block instance's bus (input pins or
    /// output pins — both are instance-inner endpoints).
    /// The endpoints a block's bus resolves to on its module's instance.
    ///
    /// **This is where a declared interface meets the module it claims to
    /// describe**, and the only place in the compiler where a name can
    /// have come from somewhere other than this crate. A `BlockInterface`
    /// built in Rust beside its module always matches; one restored from
    /// a saved file need not, and a mismatch there is a thing to report
    /// rather than a crash — so this resolves with [`try_port_id`] and
    /// names what was missing.
    fn instance_bus(
        &self,
        library: &ModuleLibrary,
        module: ModuleId,
        cell: CellId,
        bus_name: &str,
        ty: &Type,
    ) -> Result<Vec<(Word, Vec<Endpoint>)>, BlockDagError> {
        ty.word_leaves(bus_name)
            .into_iter()
            .map(|(w, names)| {
                let eps = names
                    .iter()
                    .map(|name| {
                        try_port_id(library, module, name)
                            .map(|port| Endpoint::inner(cell, port))
                            .ok_or_else(|| BlockDagError::NoSuchPort {
                                bus: bus_name.to_string(),
                                port: name.clone(),
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((w, eps))
            })
            .collect()
    }

    fn instance_input_bus(
        &self,
        library: &ModuleLibrary,
        cells: &[CellId],
        bus: &BusRef,
    ) -> Result<Vec<(Word, Vec<Endpoint>)>, BlockDagError> {
        let block = self.block(bus.block)?;
        let ty = self.require_input(bus)?;
        self.instance_bus(library, block.module, cells[bus.block], &bus.bus, ty)
    }

    fn instance_output_bus(
        &self,
        library: &ModuleLibrary,
        cells: &[CellId],
        bus: &BusRef,
    ) -> Result<Vec<(Word, Vec<Endpoint>)>, BlockDagError> {
        let block = self.block(bus.block)?;
        let ty = self.require_output(bus)?;
        self.instance_bus(library, block.module, cells[bus.block], &bus.bus, ty)
    }

    // (`boundary_input_bus` / `boundary_input_named` are free fns — see
    // below; they share the compile-scoped boundary-port cache.)

    fn boundary_output_bus(
        &self,
        m: &mut ModuleBuilder,
        ext_name: &str,
        ty: &Type,
    ) -> Vec<(Word, Vec<Endpoint>)> {
        ty.word_leaves(ext_name)
            .into_iter()
            .map(|(w, names)| {
                let eps: Vec<Endpoint> = names
                    .iter()
                    .map(|n| Endpoint::module(m.output(n)))
                    .collect();
                (w, eps)
            })
            .collect()
    }
}

/// Removes a compiled patch's `0` and `1` sources when nothing wired to
/// them.
///
/// [`BlockDag::compile`] mints both up front because it cannot know
/// whether a literal constant or a widening conversion will ask for one.
/// Each is a self-holding register, so a patch that needs neither still
/// carried two words of state — invisible in a top-level circuit, but
/// **paid again at every level** once a patch can be placed inside a
/// patch. A subpatch is a name for a sub-netlist, not a wrapper around
/// one; it should cost exactly nothing, and now does.
///
/// Safe by construction: a cell whose only wire is its own feedback
/// drives nothing, so removing it cannot change any signal.
fn drop_unused_constants(root: &mut crate::model::Module, constants: &[Endpoint]) {
    for source in constants {
        let Endpoint::Cell { cell, .. } = source else {
            continue;
        };
        let used = root.wires.iter().any(|wire| {
            matches!(wire.from, Endpoint::Cell { cell: c, .. } if c == *cell)
                && !matches!(wire.to, Endpoint::Cell { cell: c, .. } if c == *cell)
        });
        if used {
            continue;
        }
        root.wires
            .retain(|wire| !matches!(wire.to, Endpoint::Cell { cell: c, .. } if c == *cell));
        root.cells.retain(|c| c.id != *cell);
    }
}

/// A module input port for `name`, created once and reused afterwards.
/// Driving several block inputs from one boundary signal is fan-out, and a
/// second `m.input(name)` would instead be an illegal duplicate port.
fn boundary_port(
    m: &mut ModuleBuilder,
    cache: &mut Vec<(String, crate::model::PortId)>,
    name: &str,
) -> Endpoint {
    if let Some((_, id)) = cache.iter().find(|(n, _)| n == name) {
        return Endpoint::module(*id);
    }
    let id = m.input(name);
    cache.push((name.to_string(), id));
    Endpoint::module(id)
}

/// Creates module input ports from explicit `leaf_names` (LSB-0 order) and
/// regroups them into per-word-leaf source endpoints matching `ty`'s
/// structure. `leaf_names.len()` must equal `ty.width()`.
fn boundary_input_named(
    m: &mut ModuleBuilder,
    cache: &mut Vec<(String, crate::model::PortId)>,
    leaf_names: &[String],
    ty: &Type,
) -> Vec<(Word, Vec<Endpoint>)> {
    let ports: Vec<Endpoint> = leaf_names
        .iter()
        .map(|n| boundary_port(m, cache, n))
        .collect();
    let mut groups = Vec::new();
    let mut offset = 0usize;
    for (word, names) in ty.word_leaves("_") {
        let n = names.len();
        groups.push((word, ports[offset..offset + n].to_vec()));
        offset += n;
    }
    debug_assert_eq!(offset, ports.len());
    groups
}

/// Wires a source bus (endpoints are signal sources) to a sink bus
/// (endpoints are sinks), inserting per-word-leaf glue where the types
/// differ.
#[allow(clippy::too_many_arguments)]
fn wire_bus(
    m: &mut ModuleBuilder,
    stdlib: &Stdlib,
    src_ty: &Type,
    src: &[(Word, Vec<Endpoint>)],
    dst_ty: &Type,
    dst: &[(Word, Vec<Endpoint>)],
    zero: Endpoint,
    one: Endpoint,
) -> Result<(), Mismatch> {
    let plan = plan_connection(src_ty, dst_ty)?;
    debug_assert_eq!(plan.fits.len(), src.len());
    debug_assert_eq!(plan.fits.len(), dst.len());
    for (k, fit) in plan.fits.iter().enumerate() {
        let src_bits = &src[k].1;
        let dst_bits = &dst[k].1;
        match fit {
            WordFit::Direct => {
                for (s, d) in src_bits.iter().zip(dst_bits) {
                    m.wire(*s, *d);
                }
            }
            WordFit::Convert(conv) => {
                let out = build_word_convert(m, stdlib, conv, src_bits, zero, one);
                for (o, d) in out.iter().zip(dst_bits) {
                    m.wire(*o, *d);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::flatten;
    use crate::interp::FlatSimulator;
    use crate::model::PortId;
    use crate::types::Word;

    /// A straight-through module with one input bus `in` and one output bus
    /// `out`, both of type `ty` (leaf `i` wired to leaf `i`).
    fn identity_module(ty: &Type, name: &str) -> (crate::model::Module, BlockInterface) {
        let mut m = ModuleBuilder::new(name);
        let ins: Vec<PortId> = ty.leaf_names("in").iter().map(|n| m.input(n)).collect();
        let outs: Vec<PortId> = ty.leaf_names("out").iter().map(|n| m.output(n)).collect();
        for (i, o) in ins.iter().zip(&outs) {
            m.wire(Endpoint::module(*i), Endpoint::module(*o));
        }
        let module = m.finish();
        let interface = BlockInterface {
            module: module.id,
            inputs: vec![("in".to_string(), ty.clone(), RoleTag::RAW)],
            outputs: vec![("out".to_string(), ty.clone(), RoleTag::RAW)],
        };
        (module, interface)
    }

    fn drive_and_read(asset: &CircuitAsset, in_bits: u16, out_width: u16) -> u64 {
        let flat = flatten(asset.root, &asset.library).expect("flattens");
        let mut sim = FlatSimulator::new(&flat);
        for i in 0..16 {
            assert!(sim.set_input(&format!("x.{i}"), (in_bits >> i) & 1 == 1));
        }
        sim.tick();
        sim.tick();
        let mut out = 0u64;
        for i in 0..out_width {
            if sim.output(&format!("y.{i}")).expect("output y exists") {
                out |= 1u64 << i;
            }
        }
        out
    }

    /// **An interface that lies about its module is reported, not fatal.**
    ///
    /// Every `BlockInterface` this crate ships is written in Rust beside
    /// the module it describes, so its bus names cannot disagree with the
    /// module's ports. That stops holding the moment an interface can be
    /// restored from a file — and `port_id`, which the compiler used to
    /// resolve those names with, says of itself that it panics when a
    /// port is absent. A saved patch naming a port that no longer exists
    /// would have taken the editor down.
    ///
    /// Built here by widening a declared bus past the ports the module
    /// actually has, which is what a module edited after its interface
    /// was saved looks like from the compiler's side.
    #[test]
    fn an_interface_naming_a_port_the_module_lacks_is_an_error() {
        let stdlib = Stdlib::build();
        let narrow = Type::word(Word::uint(8));
        let (module, mut interface) = identity_module(&narrow, "id.narrow");
        let mut library = stdlib.library.clone();
        library.insert(module);

        // The module has `in.0..in.7`; the interface now claims sixteen.
        let wide = Type::word(Word::uint(16));
        interface.inputs = vec![("in".to_string(), wide.clone(), RoleTag::RAW)];

        let mut dag = BlockDag::new();
        let block = dag.add_block(interface);
        dag.expose_input("x", block, "in", wide);
        dag.expose_output("y", block, "out", narrow);

        match dag.compile(&stdlib, &library, 1) {
            Err(BlockDagError::NoSuchPort { bus, port }) => {
                assert_eq!(bus, "in");
                assert_eq!(port, "in.8", "the first leaf the module lacks");
            }
            Err(other) => panic!("expected NoSuchPort, got {other}"),
            Ok(_) => panic!("a bus wider than its module's ports must not compile"),
        }
    }

    #[test]
    fn connecting_mismatched_blocks_inserts_glue() {
        let stdlib = Stdlib::build();
        let word = Type::word(Word::signed_q(1, 15)); // Q1.15
        let state = Type::word(Word::signed_q(4, 12)); // Q4.12

        let (mod_a, if_a) = identity_module(&word, "id.word");
        let (mod_b, if_b) = identity_module(&state, "id.state");
        let mut library = stdlib.library.clone();
        library.insert(mod_a);
        library.insert(mod_b);

        let mut dag = BlockDag::new();
        let a = dag.add_block(if_a);
        let b = dag.add_block(if_b);
        dag.connect(a, "out", b, "in"); // Q1.15 -> Q4.12: auto right-shift-3
        dag.expose_input("x", a, "in", word.clone());
        dag.expose_output("y", b, "out", state.clone());

        let asset = dag.compile(&stdlib, &library, 1).expect("compiles");

        // The DAG must implement the Q1.15 -> Q4.12 conversion end to end:
        // value-preserving right shift by 3 (arithmetic).
        for raw in [0u16, 8, 16, 0x00FF, 0x7FFF, 0x8000, 0xFFF8, 0xC000] {
            let got = drive_and_read(&asset, raw, 16);
            let want = ((raw as i16) >> 3) as u16 as u64;
            assert_eq!(got, want, "raw {raw:#x}: got {got:#x} want {want:#x}");
        }
    }

    #[test]
    fn matching_blocks_wire_directly() {
        let stdlib = Stdlib::build();
        let word = Type::word(Word::signed_q(1, 15));
        let (mod_a, if_a) = identity_module(&word, "id.a");
        let (mod_b, if_b) = identity_module(&word, "id.b");
        let mut library = stdlib.library.clone();
        library.insert(mod_a);
        library.insert(mod_b);

        let mut dag = BlockDag::new();
        let a = dag.add_block(if_a);
        let b = dag.add_block(if_b);
        dag.connect(a, "out", b, "in");
        dag.expose_input("x", a, "in", word.clone());
        dag.expose_output("y", b, "out", word.clone());

        let asset = dag.compile(&stdlib, &library, 1).expect("compiles");
        for raw in [0u16, 1, 0x1234, 0x8001, 0xFFFF] {
            assert_eq!(drive_and_read(&asset, raw, 16), raw as u64);
        }
    }

    #[test]
    fn a_constant_drives_an_input_without_a_boundary_port() {
        let stdlib = Stdlib::build();
        let word = Type::word(Word::signed_q(1, 15));
        let (mod_a, if_a) = identity_module(&word, "id.const");
        let mut library = stdlib.library.clone();
        library.insert(mod_a);

        let mut dag = BlockDag::new();
        let a = dag.add_block(if_a);
        dag.set_constant(a, "in", 0x6000); // ≈ +0.75 in Q1.15
        dag.expose_output("y", a, "out", word.clone());

        let asset = dag
            .compile(&stdlib, &library, 1)
            .expect("a constant satisfies the single-driver rule");
        let flat = flatten(asset.root, &asset.library).expect("flattens");
        let mut sim = FlatSimulator::new(&flat);
        // No `x.*` inputs exist at all — the value is baked in.
        assert!(!sim.set_input("x.0", true));
        sim.tick();
        let mut out = 0u16;
        for i in 0..16 {
            if sim.output(&format!("y.{i}")).expect("output y exists") {
                out |= 1 << i;
            }
        }
        assert_eq!(out, 0x6000);
    }

    /// One boundary signal may drive several block inputs — fan-out is
    /// free. Exposing the same leaf names twice must reuse the port, not
    /// declare a duplicate (one enable feeding two blocks is the motivating
    /// case).
    #[test]
    fn one_boundary_signal_can_drive_two_blocks() {
        let stdlib = Stdlib::build();
        let bit = Type::word(Word::bit());
        let (mod_a, if_a) = identity_module(&bit, "id.bit.a");
        let (mod_b, if_b) = identity_module(&bit, "id.bit.b");
        let mut library = stdlib.library.clone();
        library.insert(mod_a);
        library.insert(mod_b);

        let mut dag = BlockDag::new();
        let a = dag.add_block(if_a);
        let b = dag.add_block(if_b);
        dag.expose_input_named(vec!["gate".to_string()], a, "in", bit.clone());
        dag.expose_input_named(vec!["gate".to_string()], b, "in", bit.clone());
        dag.expose_output("ya", a, "out", bit.clone());
        dag.expose_output("yb", b, "out", bit);

        let asset = dag
            .compile(&stdlib, &library, 1)
            .expect("fan-out from one boundary port compiles");
        let flat = flatten(asset.root, &asset.library).expect("flattens");
        let mut sim = FlatSimulator::new(&flat);
        for value in [true, false, true] {
            assert!(sim.set_input("gate", value));
            sim.tick();
            // A 1-bit word lowers to the bus name itself, with no index.
            assert_eq!(sim.output("ya"), Some(value));
            assert_eq!(sim.output("yb"), Some(value), "both blocks see the gate");
        }
    }

    /// One block output may also feed several boundary output ports. The
    /// destinations are distinct jacks, while the source remains one net;
    /// no duplicate NAND cell is required.
    #[test]
    fn one_block_output_can_feed_two_boundary_outputs() {
        let stdlib = Stdlib::build();
        let bit = Type::word(Word::bit());
        let (identity, interface) = identity_module(&bit, "id.bit.output_fanout");
        let mut library = stdlib.library.clone();
        library.insert(identity);

        let mut dag = BlockDag::new();
        let block = dag.add_block(interface);
        dag.expose_input("x", block, "in", bit.clone());
        dag.expose_output("left", block, "out", bit.clone());
        dag.expose_output("right", block, "out", bit);
        let asset = dag
            .compile(&stdlib, &library, 1)
            .expect("one output net may feed two boundary jacks");
        let flat = flatten(asset.root, &asset.library).expect("flattens");
        let mut sim = FlatSimulator::new(&flat);

        for value in [true, false, true] {
            assert!(sim.set_input("x", value));
            sim.tick();
            assert_eq!(sim.output("left"), Some(value));
            assert_eq!(sim.output("right"), Some(value));
        }
    }

    #[test]
    fn a_constant_and_a_connection_are_two_drivers() {
        let stdlib = Stdlib::build();
        let word = Type::word(Word::signed_q(1, 15));
        let (mod_a, if_a) = identity_module(&word, "id.a");
        let (mod_b, if_b) = identity_module(&word, "id.b");
        let mut library = stdlib.library.clone();
        library.insert(mod_a);
        library.insert(mod_b);

        let mut dag = BlockDag::new();
        let a = dag.add_block(if_a);
        let b = dag.add_block(if_b);
        dag.set_constant(a, "in", 1);
        dag.set_constant(b, "in", 1);
        dag.connect(a, "out", b, "in"); // second driver of b.in
        dag.expose_output("y", b, "out", word);
        assert_eq!(
            dag.compile(&stdlib, &library, 1),
            Err(BlockDagError::MultipleDrivers {
                block: b,
                bus: "in".to_string(),
            })
        );
    }

    #[test]
    fn undriven_input_is_rejected() {
        let stdlib = Stdlib::build();
        let word = Type::word(Word::signed_q(1, 15));
        let (mod_a, if_a) = identity_module(&word, "id.a");
        let mut library = stdlib.library.clone();
        library.insert(mod_a);

        let mut dag = BlockDag::new();
        let a = dag.add_block(if_a);
        dag.expose_output("y", a, "out", word.clone());
        // `in` of block a is never driven.
        assert_eq!(
            dag.compile(&stdlib, &library, 1),
            Err(BlockDagError::UndrivenInput {
                block: a,
                bus: "in".to_string(),
            })
        );
    }

    #[test]
    fn double_driven_input_is_rejected() {
        let stdlib = Stdlib::build();
        let word = Type::word(Word::signed_q(1, 15));
        let (mod_a, if_a) = identity_module(&word, "id.a");
        let (mod_b, if_b) = identity_module(&word, "id.b");
        let (mod_c, if_c) = identity_module(&word, "id.c");
        let mut library = stdlib.library.clone();
        library.insert(mod_a);
        library.insert(mod_b);
        library.insert(mod_c);

        let mut dag = BlockDag::new();
        let a = dag.add_block(if_a);
        let b = dag.add_block(if_b);
        let c = dag.add_block(if_c);
        dag.expose_input("x", a, "in", word.clone());
        dag.connect(a, "out", c, "in");
        dag.connect(b, "out", c, "in"); // second driver of c.in
        dag.expose_input("x2", b, "in", word.clone());
        dag.expose_output("y", c, "out", word.clone());
        assert_eq!(
            dag.compile(&stdlib, &library, 1),
            Err(BlockDagError::MultipleDrivers {
                block: c,
                bus: "in".to_string(),
            })
        );
    }

    /// A patch keeps a `0` or a `1` source when — and only when — it uses
    /// that one. Both halves matter: the first is what makes a subpatch
    /// free, and without the second the pruning would be silently
    /// deleting a driver.
    ///
    /// Each literal is checked separately rather than only "none" and
    /// "both", because an implementation that kept both whenever either
    /// was used would pass those two and still cost a wasted register in
    /// the common case of a patch pinning a single all-zero value.
    #[test]
    fn a_patch_pays_only_for_the_constants_it_uses() {
        let stdlib = Stdlib::build();
        let word = Type::word(Word::signed_q(1, 15));
        let (module, interface) = identity_module(&word, "id.word");
        let mut library = stdlib.library.clone();
        library.insert(module);

        let mut bare = BlockDag::new();
        let a = bare.add_block(interface.clone());
        bare.expose_input("x", a, "in", word.clone());
        bare.expose_output("y", a, "out", word.clone());
        let bare = bare.compile(&stdlib, &library, 1).expect("compiles");
        let bare = flatten(bare.root, &bare.library).expect("flattens");
        assert_eq!(
            bare.reg_count(),
            0,
            "a wire needs no state, so the patch holding it holds none"
        );

        // All zeros need only a `0`; all ones only a `1`; alternating
        // bits need both.
        for (literal, expected_regs) in [(0x0000, 1), (0xFFFF, 1), (0x2AAA, 2)] {
            let mut pinned = BlockDag::new();
            let a = pinned.add_block(interface.clone());
            pinned.set_constant(a, "in", literal);
            pinned.expose_output("y", a, "out", word.clone());
            let asset = pinned.compile(&stdlib, &library, 1).expect("compiles");
            let flat = flatten(asset.root, &asset.library).expect("flattens");
            assert_eq!(
                flat.reg_count(),
                expected_regs,
                "{literal:#06x} needs {expected_regs} constant source(s)"
            );

            let mut sim = FlatSimulator::new(&flat);
            sim.tick();
            let read = (0..16).fold(0_u64, |word, bit| {
                word | (u64::from(sim.output(&format!("y.{bit}")).expect("output")) << bit)
            });
            assert_eq!(read, literal, "and the literal itself survives");
        }
    }

    /// **A boundary name denotes one signal.** Fan-out — the same signal
    /// exposed onto two blocks — is legal and covered by
    /// `one_boundary_signal_can_drive_two_blocks`. Everything else that
    /// makes one leaf name mean two things is refused.
    ///
    /// All three of these compiled silently before
    /// [`BlockDag::validate_boundary`], and the third produced a working
    /// top-level patch that mis-wires the moment it is placed inside
    /// another patch, because a bus is resolved by name without regard to
    /// direction.
    #[test]
    fn a_boundary_name_cannot_mean_two_things() {
        let stdlib = Stdlib::build();
        let word = Type::word(Word::signed_q(1, 15));
        let narrow = Type::word(Word::unsigned_q(0, 15));
        let (module, interface) = identity_module(&word, "id.word");
        let (narrow_module, narrow_interface) = identity_module(&narrow, "id.narrow");
        let mut library = stdlib.library.clone();
        library.insert(module);
        library.insert(narrow_module);

        let clash = |name: &str| {
            Err(BlockDagError::BoundaryNameClash {
                name: name.to_string(),
            })
        };

        // Two inputs, one name, different widths: the second used to
        // quietly reuse the first's ports.
        let mut dag = BlockDag::new();
        let wide = dag.add_block(interface.clone());
        let thin = dag.add_block(narrow_interface.clone());
        dag.expose_input("x", wide, "in", word.clone());
        dag.expose_input("x", thin, "in", narrow.clone());
        dag.expose_output("ya", wide, "out", word.clone());
        dag.expose_output("yb", thin, "out", narrow.clone());
        assert_eq!(dag.compile(&stdlib, &library, 1), clash("x.0"));

        // Two outputs, one name: two ports, one of them unreachable.
        let mut dag = BlockDag::new();
        let a = dag.add_block(interface.clone());
        let b = dag.add_block(interface.clone());
        dag.expose_input("p", a, "in", word.clone());
        dag.expose_input("q", b, "in", word.clone());
        dag.expose_output("y", a, "out", word.clone());
        dag.expose_output("y", b, "out", word.clone());
        assert_eq!(dag.compile(&stdlib, &library, 1), clash("y.0"));

        // An input and an output under one name.
        let mut dag = BlockDag::new();
        let a = dag.add_block(interface);
        dag.expose_input("x", a, "in", word.clone());
        dag.expose_output("x", a, "out", word);
        assert_eq!(dag.compile(&stdlib, &library, 1), clash("x.0"));
    }
}
