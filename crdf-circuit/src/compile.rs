//! Flattening, validation and compilation.
//!
//! `flatten` expands the module hierarchy into a flat NAND+Reg netlist
//! using a union-find over connection points, then checks the circuit
//! rules: every wire runs source → sink, every net has exactly one
//! driver, every consumed net is driven, and every combinational cycle
//! crosses a register. `CompiledCircuit::compile` lays the netlist out
//! as a bit-sliced instruction program with *generic*, name-addressed
//! top-level inputs and outputs — nothing here is specific to any
//! application. Domain-specific conventions (such as a fixed port-naming
//! ABI) are adapters layered on top by consumer crates.

use std::collections::{BTreeMap, BTreeSet};

use uuid::Uuid;

use crate::model::{
    Cell, CellId, CellKind, CircuitAsset, Endpoint, IntrinsicDecl, MAX_FLATTENED_GATES,
    MAX_FLATTENED_REGS, MAX_HIERARCHY_DEPTH, MAX_RUNTIME_STATE, MAX_TICKS_PER_STEP, Module,
    ModuleId, ModuleLibrary, PortDirection, PortId, PortRef, WireId,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompileError {
    #[error("module {0:?} is not in the library")]
    ModuleNotFound(ModuleId),
    #[error("module {module:?}: instance cell {cell:?} exceeds the hierarchy depth limit")]
    DepthLimitExceeded { module: ModuleId, cell: CellId },
    #[error("flattened circuit exceeds {MAX_FLATTENED_GATES} NAND gates")]
    TooManyGates,
    #[error("flattened circuit exceeds {MAX_FLATTENED_REGS} registers")]
    TooManyRegs,
    #[error(
        "compiled circuit owns {0} words of state, over the {MAX_RUNTIME_STATE} limit \
         (registers plus every delay bank's ring)"
    )]
    TooMuchState(usize),
    /// A caller step must advance between one and [`MAX_TICKS_PER_STEP`]
    /// circuit ticks.
    #[error("ticks per step must be 1..={MAX_TICKS_PER_STEP}, got {0}")]
    InvalidTicksPerStep(u8),
    #[error("module {module:?}: wire {wire:?} references unknown cell")]
    UnknownCell { module: ModuleId, wire: WireId },
    #[error("module {module:?}: wire {wire:?} references unknown port")]
    UnknownPort { module: ModuleId, wire: WireId },
    #[error("module {module:?}: wire {wire:?} uses a pin that does not exist on that cell kind")]
    PortRefMismatch { module: ModuleId, wire: WireId },
    #[error("module {module:?}: wire {wire:?} must run from a signal source to a signal sink")]
    WireDirection { module: ModuleId, wire: WireId },
    #[error("module {module:?}: cell {cell:?} input pin is driven by more than one source")]
    MultipleDrivers { module: ModuleId, cell: CellId },
    #[error("top-level output port {name:?} is driven by more than one source")]
    MultipleTopDrivers { name: String },
    #[error("module {module:?}: cell {cell:?} has an undriven input pin")]
    UndrivenInput { module: ModuleId, cell: CellId },
    #[error("top-level output port {name:?} is undriven")]
    UndrivenTopOutput { name: String },
    #[error("module {module:?}: cell {cell:?} sits on a combinational cycle (no register)")]
    CombinationalCycle { module: ModuleId, cell: CellId },
    #[error("top-level port name {name:?} is declared by more than one port")]
    DuplicateTopPort { name: String },
    #[error("module {module:?}: intrinsic declaration names port {port:?}, which it does not have")]
    IntrinsicUnknownPort { module: ModuleId, port: PortId },
    #[error("module {module:?}: intrinsic declaration lists port {port:?} on the wrong side")]
    IntrinsicPortDirection { module: ModuleId, port: PortId },
    #[error(
        "module {module:?}: intrinsic declaration lists {declared} of its {ports} ports. \
         A region is only safe to skip if its declaration accounts for the whole \
         boundary; an omitted port the parent has wired crosses it unseen"
    )]
    IntrinsicPortsIncomplete {
        module: ModuleId,
        declared: usize,
        ports: usize,
    },
}

/// Where a flattened gate / register came from, for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Origin {
    pub module: ModuleId,
    pub cell: CellId,
}

/// Dense net id in a [`FlatCircuit`].
pub type NetId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlatGate {
    /// The outermost intrinsic region this gate fell inside, if any.
    /// Nested declarations are claimed by the outermost, so a gate
    /// belongs to at most one.
    pub region: Option<RegionId>,
    pub a: NetId,
    pub b: NetId,
    pub y: NetId,
    pub origin: Origin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlatReg {
    /// The intrinsic region this register fell inside, if any.
    pub region: Option<RegionId>,
    pub init: bool,
    pub d: NetId,
    pub q: NetId,
    pub origin: Origin,
}

/// Index into [`FlatCircuit::intrinsics`].
pub type RegionId = u32;

/// One occurrence of a module that declared itself an intrinsic
/// ([`crate::model::IntrinsicDecl`]), located in the flattened netlist.
///
/// The gates are still there and still authoritative — a backend with
/// no lowering for `name` runs them and gets the right answer. What
/// this adds is the ability to *not* run them: a backend that knows the
/// name can read the input nets, compute the outputs its own way, and
/// skip every gate marked with this region's id.
///
/// The engine takes no view on what any name means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatIntrinsic {
    /// The opaque key the module declared.
    pub name: String,
    /// The revision it declared, so a stale lowering can be refused.
    pub revision: u32,
    /// The module this is an occurrence of.
    pub module: ModuleId,
    /// The instance-cell chain from the root, which is what
    /// distinguishes two occurrences of the same module. `Origin`
    /// cannot: it names the innermost module a gate is *written* in,
    /// which for anything built out of smaller pieces is one of those
    /// pieces and never the whole.
    pub path: Vec<CellId>,
    /// Nets carrying the declared inputs, in declared order. Two
    /// entries may be equal, when the surrounding circuit tied two
    /// inputs together.
    pub inputs: Vec<NetId>,
    /// Nets carrying the declared outputs, in declared order.
    pub outputs: Vec<NetId>,
    /// The key a backend registers a lowering against, or `None` when
    /// the module cannot be flattened on its own and so cannot be
    /// keyed. See `module_digest`.
    pub digest: Option<Uuid>,
    /// Whether any register fell inside. A lowering for a region with
    /// state has to carry that state itself; one for a region without
    /// is a pure function of its inputs.
    pub has_state: bool,
}

/// A flattened, validated netlist. Gates are in evaluation (topological)
/// order. Top-level ports are exposed by name so that any caller (an
/// embedder's own port convention, tests, tools) can drive arbitrary
/// circuits.
///
/// Fields are crate-private on purpose: a `FlatCircuit` can only come
/// out of [`flatten`], so the invariants (`from_flat`, the simulator)
/// rely on are guaranteed by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatCircuit {
    pub(crate) net_count: u32,
    /// Topologically ordered.
    pub(crate) gates: Vec<FlatGate>,
    pub(crate) regs: Vec<FlatReg>,
    /// `(port name, net)` for each top-level input port.
    pub(crate) inputs: Vec<(String, NetId)>,
    /// `(port name, net)` for each top-level output port.
    pub(crate) outputs: Vec<(String, NetId)>,
    /// Occurrences of modules that declared themselves substitutable.
    /// Empty unless some module in the tree carried a declaration.
    pub(crate) intrinsics: Vec<FlatIntrinsic>,
}

impl FlatCircuit {
    pub fn gate_count(&self) -> usize {
        self.gates.len()
    }

    pub fn reg_count(&self) -> usize {
        self.regs.len()
    }

    /// Occurrences of modules that declared themselves substitutable
    /// (`crate::model::IntrinsicDecl`), in the order flattening met
    /// them. Empty for a circuit with no declarations, which is every
    /// circuit until someone writes one.
    pub fn intrinsics(&self) -> &[FlatIntrinsic] {
        &self.intrinsics
    }

    /// The gates, so a backend can see which region each one fell into.
    pub fn gates(&self) -> &[FlatGate] {
        &self.gates
    }

    /// The registers, likewise.
    pub fn regs(&self) -> &[FlatReg] {
        &self.regs
    }

    /// Top-level input port names, in declaration order.
    pub fn input_names(&self) -> impl Iterator<Item = &str> {
        self.inputs.iter().map(|(name, _)| name.as_str())
    }

    /// Top-level output port names, in declaration order.
    pub fn output_names(&self) -> impl Iterator<Item = &str> {
        self.outputs.iter().map(|(name, _)| name.as_str())
    }
}

/// Which top-level inputs each top-level output depends on **within one
/// tick** — the module's combinational shape, read off its netlist.
///
/// A directed cycle is legal exactly when it crosses a register (the
/// delayed-trace guard). Deciding that per *module* rather than per
/// *signal* means asking whether a whole block is "a delay", which is a
/// question with no general answer: a state machine can hold its state
/// and pass one input straight through, and a module somebody assembles
/// today cannot be on anyone's list.
///
/// So it is derived. An output that is reached only through a register
/// depends on nothing this tick, and is exactly as safe to close a loop
/// through as `std.delay` is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombinationalDeps {
    /// `(output port, the input ports it depends on this tick)`, both in
    /// declaration order.
    pub outputs: Vec<(String, Vec<String>)>,
}

impl CombinationalDeps {
    /// Whether `output` changes within the tick that `input` changes.
    pub fn depends(&self, output: &str, input: &str) -> bool {
        self.outputs
            .iter()
            .find(|(name, _)| name == output)
            .is_some_and(|(_, inputs)| inputs.iter().any(|name| name == input))
    }
}

/// Reads [`CombinationalDeps`] off a module by flattening it alone.
///
/// Walks back from each output through gates and **stops at a register**:
/// a register's output is last tick's value, so nothing upstream of it is
/// a same-tick dependency.
pub fn combinational_deps(
    root: ModuleId,
    library: &ModuleLibrary,
) -> Result<CombinationalDeps, CompileError> {
    let flat = flatten(root, library)?;

    // Which gate drives each net, and which nets a register drives.
    let mut driver: BTreeMap<NetId, usize> = BTreeMap::new();
    for (index, gate) in flat.gates.iter().enumerate() {
        driver.insert(gate.y, index);
    }
    let latched: BTreeSet<NetId> = flat.regs.iter().map(|reg| reg.q).collect();
    let input_of: BTreeMap<NetId, &str> = flat
        .inputs
        .iter()
        .map(|(name, net)| (*net, name.as_str()))
        .collect();

    let outputs = flat
        .outputs
        .iter()
        .map(|(name, net)| {
            let mut reached: BTreeSet<&str> = BTreeSet::new();
            let mut seen: BTreeSet<NetId> = BTreeSet::new();
            let mut stack = vec![*net];
            while let Some(net) = stack.pop() {
                if !seen.insert(net) {
                    continue;
                }
                if let Some(input) = input_of.get(&net) {
                    reached.insert(input);
                    // An input net can also be a constant's; nothing
                    // upstream either way.
                    continue;
                }
                // A register's output is last tick's, so the walk ends.
                if latched.contains(&net) {
                    continue;
                }
                if let Some(gate) = driver.get(&net).map(|index| flat.gates[*index]) {
                    stack.push(gate.a);
                    stack.push(gate.b);
                }
            }
            // Declaration order, so the answer is stable to compare.
            let inputs = flat
                .inputs
                .iter()
                .filter(|(name, _)| reached.contains(name.as_str()))
                .map(|(name, _)| name.clone())
                .collect();
            (name.clone(), inputs)
        })
        .collect();

    Ok(CombinationalDeps { outputs })
}

/// A hierarchical map from a model signal — addressed by an instance
/// path plus a model [`Endpoint`] — to its dense [`NetId`] in the
/// flattened netlist.
///
/// Because the same module can be instantiated many times, a signal is
/// identified by the *path* of instance cells from the root down to the
/// module that owns the endpoint (an empty path = the root module
/// itself), together with the endpoint within that module. This is the
/// generic, domain-agnostic address the probe / live-signal tooling
/// needs; it is produced only on request by [`flatten_with_net_map`] so
/// the ordinary [`flatten`] and [`FlatCircuit`] stay minimal.
///
/// A returned `NetId` indexes the unfolded reference interpreter
/// ([`crate::interp::FlatSimulator`]); the bit-sliced / JIT evaluators
/// fold non-observed nets away and cannot be probed this way.
#[derive(Debug, Clone)]
pub struct HierNetMap {
    root: ModuleId,
    occurrences: BTreeMap<Box<[CellId]>, OccurrenceNets>,
}

#[derive(Debug, Clone)]
struct OccurrenceNets {
    module: ModuleId,
    /// This occurrence's own boundary ports. For an instance cell these
    /// same nets are reachable from the parent as `Inner` pins.
    module_ports: BTreeMap<PortId, NetId>,
    cells: BTreeMap<CellId, CellNets>,
}

#[derive(Debug, Clone, Copy)]
enum CellNets {
    Nand {
        a: Option<NetId>,
        b: Option<NetId>,
        y: Option<NetId>,
    },
    Reg {
        d: Option<NetId>,
        q: Option<NetId>,
    },
    /// An instance cell's pins live in the child occurrence at
    /// `path + [cell]`, so no nets are stored here.
    Instance,
}

impl HierNetMap {
    /// The root module the map was built for.
    pub fn root(&self) -> ModuleId {
        self.root
    }

    /// The module instantiated at `instance_path` (empty path = root),
    /// if that occurrence exists.
    pub fn module_at(&self, instance_path: &[CellId]) -> Option<ModuleId> {
        self.occurrences.get(instance_path).map(|o| o.module)
    }

    /// The net driving/observing `endpoint` inside the module reached by
    /// `instance_path`. Returns `None` when the path or endpoint is
    /// unknown, or when the net is unobservable (e.g. an unconnected
    /// instance output that no gate, register or port references).
    pub fn net(&self, instance_path: &[CellId], endpoint: Endpoint) -> Option<NetId> {
        let occ = self.occurrences.get(instance_path)?;
        match endpoint {
            Endpoint::Module { port } => occ.module_ports.get(&port).copied(),
            Endpoint::Cell { cell, port } => match occ.cells.get(&cell)? {
                CellNets::Nand { a, b, y } => match port {
                    PortRef::NandA => *a,
                    PortRef::NandB => *b,
                    PortRef::NandY => *y,
                    _ => None,
                },
                CellNets::Reg { d, q } => match port {
                    PortRef::RegD => *d,
                    PortRef::RegQ => *q,
                    _ => None,
                },
                CellNets::Instance => match port {
                    // An instance pin is the child occurrence's like-named
                    // boundary port (they share the same connection point).
                    PortRef::Inner(inner) => {
                        let mut child = instance_path.to_vec();
                        child.push(cell);
                        self.occurrences
                            .get(child.as_slice())?
                            .module_ports
                            .get(&inner)
                            .copied()
                    }
                    _ => None,
                },
            },
        }
    }
}

/// Per-occurrence connection points recorded during flattening (before
/// union-find is collapsed to dense net ids).
struct OccurrenceRec {
    path: Vec<CellId>,
    module: ModuleId,
    module_ports: Vec<(PortId, u32)>,
    cells: Vec<(CellId, CellPointRef)>,
}

#[derive(Clone, Copy)]
enum CellPointRef {
    Nand { a: u32, b: u32, y: u32 },
    Reg { d: u32, q: u32 },
    Instance,
}

/// Union-find over connection points.
struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    fn new() -> Self {
        Self { parent: Vec::new() }
    }

    fn fresh(&mut self) -> u32 {
        let id = self.parent.len() as u32;
        self.parent.push(id);
        id
    }

    fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let grandparent = self.parent[self.parent[x as usize] as usize];
            self.parent[x as usize] = grandparent;
            x = grandparent;
        }
        x
    }

    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            // Deterministic: smaller root wins.
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent[hi as usize] = lo;
        }
    }
}

/// Pin points allocated for one cell occurrence.
enum CellPins {
    Nand { a: u32, b: u32, y: u32 },
    Reg { d: u32, q: u32 },
    Instance { ports: BTreeMap<PortId, u32> },
}

struct PendingRegion {
    digest: Option<Uuid>,
    name: String,
    revision: u32,
    module: ModuleId,
    path: Vec<CellId>,
    /// Connection points, pre-union-find, in declared order.
    inputs: Vec<u32>,
    outputs: Vec<u32>,
    has_state: bool,
}

struct PendingGate {
    region: Option<RegionId>,
    a: u32,
    b: u32,
    y: u32,
    origin: Origin,
}

struct PendingReg {
    region: Option<RegionId>,
    init: bool,
    d: u32,
    q: u32,
    origin: Origin,
}

/// A consumed point together with diagnostics for undriven reporting.
enum SinkKind {
    CellInput(Origin),
    TopOutput(usize),
}

struct Flattener<'a> {
    library: &'a ModuleLibrary,
    uf: UnionFind,
    gates: Vec<PendingGate>,
    regs: Vec<PendingReg>,
    /// The intrinsic region currently being expanded, if any. A nested
    /// declaration inside an open one is ignored: the outermost claims
    /// the gates, because a backend that substitutes for the whole has
    /// no use for a substitute for a part of it.
    open_region: Option<RegionId>,
    /// One entry per occurrence of a module that declared itself
    /// substitutable. Port points are recorded raw here and resolved to
    /// nets once union-find has settled.
    regions: Vec<PendingRegion>,
    /// False while computing a region digest, which flattens a module
    /// on its own -- a module that declares itself would otherwise ask
    /// for its own digest forever.
    record_regions: bool,
    /// One digest per declared module, so repeated occurrences of the
    /// same module are hashed once.
    digest_cache: BTreeMap<ModuleId, Option<Uuid>>,
    /// `Some` when a hierarchical net map is being recorded (opt-in).
    probe: Option<Vec<OccurrenceRec>>,
}

impl<'a> Flattener<'a> {
    /// Expands `module_id`, whose own ports are already bound to the
    /// given points, into `self`. `path` is the instance-cell chain from
    /// the root to this occurrence (empty at the root).
    fn instantiate(
        &mut self,
        module_id: ModuleId,
        depth: usize,
        port_points: &BTreeMap<PortId, u32>,
        path: &[CellId],
    ) -> Result<(), CompileError> {
        let module = self
            .library
            .get(module_id)
            .ok_or(CompileError::ModuleNotFound(module_id))?;

        // Open an intrinsic region, if this module declared itself one
        // and we are not already inside another. The outermost claims
        // the gates: a backend that can compute the whole has no use for
        // a substitute for a part of it.
        let opened = if self.open_region.is_none() {
            match &module.intrinsic {
                Some(decl) => {
                    let resolve_port =
                        |wanted: PortId, want: PortDirection| -> Result<u32, CompileError> {
                            let port =
                                module
                                    .port(wanted)
                                    .ok_or(CompileError::IntrinsicUnknownPort {
                                        module: module_id,
                                        port: wanted,
                                    })?;
                            if port.direction != want {
                                return Err(CompileError::IntrinsicPortDirection {
                                    module: module_id,
                                    port: wanted,
                                });
                            }
                            port_points.get(&wanted).copied().ok_or(
                                CompileError::IntrinsicUnknownPort {
                                    module: module_id,
                                    port: wanted,
                                },
                            )
                        };
                    // **Every port, exactly once.** A region is only
                    // safe to skip if its declaration accounts for the
                    // whole boundary. Hierarchy guarantees a net inside
                    // a module reaches the outside only through that
                    // module's ports -- but it says nothing about
                    // whether the *declaration* listed them all, and an
                    // omitted port that the parent has wired is a
                    // signal crossing the region unseen. A backend
                    // skipping the gates would then read an input it
                    // was never handed, or leave an output no one
                    // computes.
                    let mut declared: Vec<PortId> = decl.inputs.clone();
                    declared.extend(decl.outputs.iter().copied());
                    let mut sorted = declared.clone();
                    sorted.sort();
                    sorted.dedup();
                    if sorted.len() != declared.len() || declared.len() != module.ports.len() {
                        return Err(CompileError::IntrinsicPortsIncomplete {
                            module: module_id,
                            declared: declared.len(),
                            ports: module.ports.len(),
                        });
                    }
                    for port in &module.ports {
                        if !declared.contains(&port.id) {
                            return Err(CompileError::IntrinsicUnknownPort {
                                module: module_id,
                                port: port.id,
                            });
                        }
                    }
                    let inputs = decl
                        .inputs
                        .iter()
                        .map(|p| resolve_port(*p, PortDirection::Input))
                        .collect::<Result<Vec<_>, _>>()?;
                    let outputs = decl
                        .outputs
                        .iter()
                        .map(|p| resolve_port(*p, PortDirection::Output))
                        .collect::<Result<Vec<_>, _>>()?;
                    // Hashed once per declared module, however many
                    // times it occurs; suppressed entirely while a
                    // digest is itself being computed.
                    let digest = if self.record_regions {
                        match self.digest_cache.get(&module_id) {
                            Some(cached) => *cached,
                            None => {
                                let computed = module_digest(module_id, self.library, decl);
                                self.digest_cache.insert(module_id, computed);
                                computed
                            }
                        }
                    } else {
                        None
                    };
                    let id = u32::try_from(self.regions.len())
                        .map_err(|_| CompileError::TooManyGates)?;
                    self.regions.push(PendingRegion {
                        name: decl.name.clone(),
                        revision: decl.revision,
                        module: module_id,
                        path: path.to_vec(),
                        inputs,
                        outputs,
                        digest,
                        has_state: false,
                    });
                    self.open_region = Some(id);
                    Some(id)
                }
                None => None,
            }
        } else {
            None
        };

        let mut cell_pins: BTreeMap<CellId, CellPins> = BTreeMap::new();
        for cell in &module.cells {
            let origin = Origin {
                module: module_id,
                cell: cell.id,
            };
            match cell.kind {
                CellKind::Nand => {
                    if self.gates.len() >= MAX_FLATTENED_GATES {
                        return Err(CompileError::TooManyGates);
                    }
                    let (a, b, y) = (self.uf.fresh(), self.uf.fresh(), self.uf.fresh());
                    self.gates.push(PendingGate {
                        a,
                        b,
                        y,
                        origin,
                        region: self.open_region,
                    });
                    cell_pins.insert(cell.id, CellPins::Nand { a, b, y });
                }
                CellKind::Reg { init } => {
                    if self.regs.len() >= MAX_FLATTENED_REGS {
                        return Err(CompileError::TooManyRegs);
                    }
                    let (d, q) = (self.uf.fresh(), self.uf.fresh());
                    self.regs.push(PendingReg {
                        init,
                        d,
                        q,
                        origin,
                        region: self.open_region,
                    });
                    if let Some(region) = self.open_region {
                        self.regions[region as usize].has_state = true;
                    }
                    cell_pins.insert(cell.id, CellPins::Reg { d, q });
                }
                CellKind::Instance { module: inner_id } => {
                    if depth + 1 > MAX_HIERARCHY_DEPTH {
                        return Err(CompileError::DepthLimitExceeded {
                            module: module_id,
                            cell: cell.id,
                        });
                    }
                    let inner = self
                        .library
                        .get(inner_id)
                        .ok_or(CompileError::ModuleNotFound(inner_id))?;
                    let ports: BTreeMap<PortId, u32> = inner
                        .ports
                        .iter()
                        .map(|port| (port.id, self.uf.fresh()))
                        .collect();
                    let mut child = path.to_vec();
                    child.push(cell.id);
                    self.instantiate(inner_id, depth + 1, &ports, &child)?;
                    cell_pins.insert(cell.id, CellPins::Instance { ports });
                }
            }
        }

        // Record this occurrence's connection points before union-find
        // collapses them (only when a net map was requested).
        if let Some(recs) = &mut self.probe {
            let cells = module
                .cells
                .iter()
                .map(|cell| {
                    let point_ref = match cell_pins.get(&cell.id).expect("pins built above") {
                        CellPins::Nand { a, b, y } => CellPointRef::Nand {
                            a: *a,
                            b: *b,
                            y: *y,
                        },
                        CellPins::Reg { d, q } => CellPointRef::Reg { d: *d, q: *q },
                        CellPins::Instance { .. } => CellPointRef::Instance,
                    };
                    (cell.id, point_ref)
                })
                .collect();
            recs.push(OccurrenceRec {
                path: path.to_vec(),
                module: module_id,
                module_ports: port_points.iter().map(|(p, pt)| (*p, *pt)).collect(),
                cells,
            });
        }

        for wire in &module.wires {
            let from = self.resolve(module, wire.id, wire.from, &cell_pins, port_points)?;
            let to = self.resolve(module, wire.id, wire.to, &cell_pins, port_points)?;
            if !from.is_source || to.is_source {
                return Err(CompileError::WireDirection {
                    module: module_id,
                    wire: wire.id,
                });
            }
            self.uf.union(from.point, to.point);
        }
        if opened.is_some() {
            self.open_region = None;
        }
        Ok(())
    }

    /// Resolves one endpoint to its connection point and classifies it
    /// as source or sink.
    fn resolve(
        &mut self,
        module: &Module,
        wire: WireId,
        endpoint: Endpoint,
        cell_pins: &BTreeMap<CellId, CellPins>,
        port_points: &BTreeMap<PortId, u32>,
    ) -> Result<Resolved, CompileError> {
        let module_id = module.id;
        match endpoint {
            Endpoint::Module { port } => {
                let declared = module.port(port).ok_or(CompileError::UnknownPort {
                    module: module_id,
                    wire,
                })?;
                let point = *port_points.get(&port).ok_or(CompileError::UnknownPort {
                    module: module_id,
                    wire,
                })?;
                // Seen from inside the module, an input port supplies
                // the signal and an output port consumes it.
                Ok(Resolved {
                    point,
                    is_source: declared.direction == PortDirection::Input,
                })
            }
            Endpoint::Cell { cell, port } => {
                let pins = cell_pins.get(&cell).ok_or(CompileError::UnknownCell {
                    module: module_id,
                    wire,
                })?;
                let mismatch = CompileError::PortRefMismatch {
                    module: module_id,
                    wire,
                };
                match (pins, port) {
                    (CellPins::Nand { a, .. }, PortRef::NandA) => Ok(Resolved {
                        point: *a,
                        is_source: false,
                    }),
                    (CellPins::Nand { b, .. }, PortRef::NandB) => Ok(Resolved {
                        point: *b,
                        is_source: false,
                    }),
                    (CellPins::Nand { y, .. }, PortRef::NandY) => Ok(Resolved {
                        point: *y,
                        is_source: true,
                    }),
                    (CellPins::Reg { d, .. }, PortRef::RegD) => Ok(Resolved {
                        point: *d,
                        is_source: false,
                    }),
                    (CellPins::Reg { q, .. }, PortRef::RegQ) => Ok(Resolved {
                        point: *q,
                        is_source: true,
                    }),
                    (CellPins::Instance { ports }, PortRef::Inner(inner_port)) => {
                        let Cell {
                            kind: CellKind::Instance { module: inner_id },
                            ..
                        } = module.cell(cell).expect("pins imply cell exists")
                        else {
                            unreachable!("instance pins imply instance cell");
                        };
                        let inner = self
                            .library
                            .get(*inner_id)
                            .ok_or(CompileError::ModuleNotFound(*inner_id))?;
                        let declared = inner.port(inner_port).ok_or(CompileError::UnknownPort {
                            module: module_id,
                            wire,
                        })?;
                        let point = *ports.get(&inner_port).ok_or(mismatch)?;
                        // Seen from the parent, an instance's input port
                        // consumes the signal and its output supplies it.
                        Ok(Resolved {
                            point,
                            is_source: declared.direction == PortDirection::Output,
                        })
                    }
                    _ => Err(mismatch),
                }
            }
        }
    }
}

struct Resolved {
    point: u32,
    is_source: bool,
}

/// Flattens and validates `root` against the circuit rules. Top-level
/// port names are exposed as-is, and stay addressable by name through
/// [`CompiledCircuit::compile`].
pub fn flatten(root: ModuleId, library: &ModuleLibrary) -> Result<FlatCircuit, CompileError> {
    Ok(flatten_impl(root, library, false, true)?.0)
}

/// Like [`flatten`], but also returns a [`HierNetMap`] addressing every
/// model signal by instance path + endpoint. Opt-in: the extra
/// bookkeeping is only done here, so the common [`flatten`] path stays
/// minimal.
pub fn flatten_with_net_map(
    root: ModuleId,
    library: &ModuleLibrary,
) -> Result<(FlatCircuit, HierNetMap), CompileError> {
    let (flat, map) = flatten_impl(root, library, true, true)?;
    Ok((flat, map.expect("net map requested")))
}

fn flatten_impl(
    root: ModuleId,
    library: &ModuleLibrary,
    record: bool,
    record_regions: bool,
) -> Result<(FlatCircuit, Option<HierNetMap>), CompileError> {
    let root_module = library
        .get(root)
        .ok_or(CompileError::ModuleNotFound(root))?;

    let mut flattener = Flattener {
        library,
        uf: UnionFind::new(),
        gates: Vec::new(),
        regs: Vec::new(),
        open_region: None,
        regions: Vec::new(),
        record_regions,
        digest_cache: BTreeMap::new(),
        probe: record.then(Vec::new),
    };

    let root_ports: BTreeMap<PortId, u32> = root_module
        .ports
        .iter()
        .map(|port| (port.id, flattener.uf.fresh()))
        .collect();
    flattener.instantiate(root, 0, &root_ports, &[])?;

    let Flattener {
        regions,
        mut uf,
        gates,
        regs,
        probe,
        ..
    } = flattener;

    // Dense net numbering over union-find roots, in point order (which
    // is allocation order and therefore deterministic).
    let mut net_of_root: BTreeMap<u32, NetId> = BTreeMap::new();
    let mut net_for = |uf: &mut UnionFind, point: u32, next: &mut u32| -> NetId {
        let root_point = uf.find(point);
        *net_of_root.entry(root_point).or_insert_with(|| {
            let id = *next;
            *next += 1;
            id
        })
    };
    let mut net_count: u32 = 0;

    let gates: Vec<FlatGate> = gates
        .into_iter()
        .map(|gate| FlatGate {
            a: net_for(&mut uf, gate.a, &mut net_count),
            b: net_for(&mut uf, gate.b, &mut net_count),
            y: net_for(&mut uf, gate.y, &mut net_count),
            origin: gate.origin,
            region: gate.region,
        })
        .collect();
    let regs: Vec<FlatReg> = regs
        .into_iter()
        .map(|reg| FlatReg {
            init: reg.init,
            d: net_for(&mut uf, reg.d, &mut net_count),
            q: net_for(&mut uf, reg.q, &mut net_count),
            origin: reg.origin,
            region: reg.region,
        })
        .collect();

    // Region ports, resolved to nets now that union-find has settled.
    // Two entries can come out equal, which is not a fault: the
    // surrounding circuit is allowed to tie two of a region's inputs
    // to the same signal.
    let intrinsics: Vec<FlatIntrinsic> = regions
        .into_iter()
        .map(|region| FlatIntrinsic {
            name: region.name,
            revision: region.revision,
            module: region.module,
            path: region.path,
            inputs: region
                .inputs
                .into_iter()
                .map(|p| net_for(&mut uf, p, &mut net_count))
                .collect(),
            outputs: region
                .outputs
                .into_iter()
                .map(|p| net_for(&mut uf, p, &mut net_count))
                .collect(),
            digest: region.digest,
            has_state: region.has_state,
        })
        .collect();

    let mut inputs: Vec<(String, NetId)> = Vec::new();
    let mut outputs: Vec<(String, NetId)> = Vec::new();
    for port in &root_module.ports {
        let net = net_for(&mut uf, root_ports[&port.id], &mut net_count);
        match port.direction {
            PortDirection::Input => inputs.push((port.name.clone(), net)),
            PortDirection::Output => outputs.push((port.name.clone(), net)),
        }
    }

    // Finalize the opt-in net map now that every referenced net has a
    // dense id. Points whose net is never referenced (e.g. an unconnected
    // instance output) map to `None` — genuinely unobservable.
    let net_map = probe.map(|recs| {
        let lookup = |uf: &mut UnionFind, point: u32| net_of_root.get(&uf.find(point)).copied();
        let mut occurrences: BTreeMap<Box<[CellId]>, OccurrenceNets> = BTreeMap::new();
        for rec in recs {
            let module_ports = rec
                .module_ports
                .into_iter()
                .filter_map(|(port, point)| lookup(&mut uf, point).map(|net| (port, net)))
                .collect();
            let cells = rec
                .cells
                .into_iter()
                .map(|(id, point_ref)| {
                    let nets = match point_ref {
                        CellPointRef::Nand { a, b, y } => CellNets::Nand {
                            a: lookup(&mut uf, a),
                            b: lookup(&mut uf, b),
                            y: lookup(&mut uf, y),
                        },
                        CellPointRef::Reg { d, q } => CellNets::Reg {
                            d: lookup(&mut uf, d),
                            q: lookup(&mut uf, q),
                        },
                        CellPointRef::Instance => CellNets::Instance,
                    };
                    (id, nets)
                })
                .collect();
            occurrences.insert(
                rec.path.into_boxed_slice(),
                OccurrenceNets {
                    module: rec.module,
                    module_ports,
                    cells,
                },
            );
        }
        HierNetMap { root, occurrences }
    });

    // Exactly one driver per net; every consumed net driven.
    let mut driver_count = vec![0_u32; net_count as usize];
    let mut driver_example: Vec<Option<Origin>> = vec![None; net_count as usize];
    for gate in &gates {
        driver_count[gate.y as usize] += 1;
        driver_example[gate.y as usize] = Some(gate.origin);
    }
    for reg in &regs {
        driver_count[reg.q as usize] += 1;
        driver_example[reg.q as usize] = Some(reg.origin);
    }
    for (_, net) in &inputs {
        driver_count[*net as usize] += 1;
    }

    let mut sinks: Vec<(NetId, SinkKind)> = Vec::new();
    for gate in &gates {
        sinks.push((gate.a, SinkKind::CellInput(gate.origin)));
        sinks.push((gate.b, SinkKind::CellInput(gate.origin)));
    }
    for reg in &regs {
        sinks.push((reg.d, SinkKind::CellInput(reg.origin)));
    }
    for (index, (_, net)) in outputs.iter().enumerate() {
        sinks.push((*net, SinkKind::TopOutput(index)));
    }

    for (net, sink) in &sinks {
        match driver_count[*net as usize] {
            1 => {}
            0 => {
                return Err(match sink {
                    SinkKind::CellInput(origin) => CompileError::UndrivenInput {
                        module: origin.module,
                        cell: origin.cell,
                    },
                    SinkKind::TopOutput(index) => CompileError::UndrivenTopOutput {
                        name: outputs[*index].0.clone(),
                    },
                });
            }
            _ => {
                return Err(match sink {
                    SinkKind::CellInput(origin) => CompileError::MultipleDrivers {
                        module: origin.module,
                        cell: origin.cell,
                    },
                    SinkKind::TopOutput(index) => CompileError::MultipleTopDrivers {
                        name: outputs[*index].0.clone(),
                    },
                });
            }
        }
    }
    // Nets driven twice but never consumed are still malformed. Such a
    // net may have only top-input drivers (two inputs shorted into an
    // unused instance pin), so fall back to the input name.
    for (net, count) in driver_count.iter().enumerate() {
        if *count > 1 {
            return Err(match driver_example[net] {
                Some(origin) => CompileError::MultipleDrivers {
                    module: origin.module,
                    cell: origin.cell,
                },
                None => {
                    let name = inputs
                        .iter()
                        .find(|(_, input_net)| *input_net as usize == net)
                        .map(|(name, _)| name.clone())
                        .unwrap_or_default();
                    CompileError::MultipleTopDrivers { name }
                }
            });
        }
    }

    // Kahn topological sort over gates. Register outputs and top inputs
    // are free; only gate-produced nets create dependencies.
    let mut producer: Vec<Option<u32>> = vec![None; net_count as usize];
    for (index, gate) in gates.iter().enumerate() {
        producer[gate.y as usize] = Some(index as u32);
    }
    let mut indegree = vec![0_u32; gates.len()];
    let mut consumers: Vec<Vec<u32>> = vec![Vec::new(); gates.len()];
    for (index, gate) in gates.iter().enumerate() {
        let mut depend_on = |net: NetId| {
            if let Some(prod) = producer[net as usize] {
                indegree[index] += 1;
                consumers[prod as usize].push(index as u32);
            }
        };
        depend_on(gate.a);
        if gate.b != gate.a {
            depend_on(gate.b);
        }
    }
    let mut queue: std::collections::VecDeque<u32> = (0..gates.len() as u32)
        .filter(|&index| indegree[index as usize] == 0)
        .collect();
    let mut order: Vec<u32> = Vec::with_capacity(gates.len());
    while let Some(index) = queue.pop_front() {
        order.push(index);
        for &consumer in &consumers[index as usize] {
            indegree[consumer as usize] -= 1;
            if indegree[consumer as usize] == 0 {
                queue.push_back(consumer);
            }
        }
    }
    if order.len() != gates.len() {
        let stuck = (0..gates.len())
            .find(|&index| indegree[index] > 0)
            .expect("incomplete topo order implies a stuck gate");
        return Err(CompileError::CombinationalCycle {
            module: gates[stuck].origin.module,
            cell: gates[stuck].origin.cell,
        });
    }
    let gates: Vec<FlatGate> = order.into_iter().map(|i| gates[i as usize]).collect();

    Ok((
        FlatCircuit {
            intrinsics,
            net_count,
            gates,
            regs,
            inputs,
            outputs,
        },
        net_map,
    ))
}

/// One bit-sliced instruction over `u64` lanes. `FullAdder` is
/// produced by the folding pass (see [`crate::fold`]): it evaluates
/// with the generate/propagate form, replacing nine NANDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Instr {
    Nand {
        a: u32,
        b: u32,
        out: u32,
    },
    FullAdder {
        a: u32,
        b: u32,
        cin: u32,
        sum: u32,
        cout: u32,
    },
    Xor {
        a: u32,
        b: u32,
        out: u32,
    },
    And {
        a: u32,
        b: u32,
        out: u32,
    },
    Not {
        a: u32,
        out: u32,
    },
    /// An intrinsic region, as one instruction.
    ///
    /// The index selects a [`CompiledRegion`], which carries both the
    /// key a backend looks up and the instructions to run when the
    /// lookup finds nothing. An evaluator that knows no lowerings
    /// simply runs the fallback and is right.
    Region {
        region: u32,
    },
}

/// One register a region owns: where its current value is read from and
/// where its next value has to be written.
///
/// `q_slot` holds what the register latched last tick, loaded from
/// register state before anything runs. `d_slot` is where this tick's
/// answer goes; the ordinary latch at the end of the tick picks it up,
/// so a substitute writes next state exactly where the gates would
/// have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateBinding {
    pub q_slot: u32,
    pub d_slot: u32,
    /// What the register holds after a reset.
    pub init: bool,
}

/// An intrinsic region, compiled: what to look a lowering up by, where
/// its values live, and what to do without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledRegion {
    /// The opaque key the module declared.
    pub name: String,
    /// The revision it declared.
    pub revision: u32,
    /// A hash of the fallback, so a netlist that changed without its
    /// revision being bumped retires any lowering registered against
    /// it. Two circuits agreeing here computed the region the same way.
    pub digest: Uuid,
    /// Slots holding the declared inputs, in declared order.
    pub inputs: Box<[u32]>,
    /// Slots the region must leave holding the declared outputs.
    pub outputs: Box<[u32]>,
    /// Registers the region owns, in flatten order. A lowering has to
    /// return one next-state value for each, and gets the current value
    /// of each. Empty for a region that holds nothing but constants.
    pub state: Box<[StateBinding]>,
    /// Whether nothing outside the region reads any of that state.
    ///
    /// When true a backend may keep it in whatever form it likes
    /// between ticks, because no one else can tell the difference.
    /// When false the state has to go back into the register array
    /// every tick, in the representation everything else reads.
    pub state_is_private: bool,
    /// The region's own gates, folded. Not a second definition of the
    /// region -- the definition -- so a backend without a lowering runs
    /// these and cannot be wrong.
    pub(crate) fallback: Box<[Instr]>,
}

impl CompiledRegion {
    /// How many instructions the fallback runs. The measure of what a
    /// lowering removes.
    pub fn fallback_len(&self) -> usize {
        self.fallback.len()
    }

    /// What its gates cost, in bitwise operations.
    ///
    /// The thing to weigh a substitution against. A backend that
    /// replaces a region pays for the boundary it crosses, and a region
    /// whose gates are cheap relative to that boundary is *slower*
    /// substituted -- so whoever offers a replacement wants both
    /// numbers, and wants them before it offers.
    pub fn fallback_ops(&self) -> usize {
        self.fallback.iter().map(op_weight).sum()
    }

    /// How many bit planes cross its boundary each tick.
    ///
    /// Inputs in and outputs out — and state **both ways** unless the
    /// region holds it privately. Private state is the whole reason
    /// `state_is_private` is tracked: a region nothing else reads may
    /// keep its registers in whatever form it computes in and never
    /// move them across at all. A region whose state is read from
    /// outside gets the current value handed in and has to hand the
    /// next one back, so it pays for both directions.
    pub fn boundary_planes(&self) -> usize {
        let state = if self.state_is_private {
            0
        } else {
            2 * self.state.len()
        };
        self.inputs.len() + self.outputs.len() + state
    }
}

/// A canonical hash of what a declared module computes.
///
/// Its job is to notice that a netlist changed while its declared
/// revision did not, so a lowering registered against the old gates
/// stops being used.
///
/// It is taken from **the module flattened on its own**, not from the
/// module as it appears inside some larger circuit. That distinction
/// was the whole difficulty. A first version hashed the folded
/// fallback, which reads well — it is exactly what a backend would
/// otherwise run — and is wrong: the fallback's instruction order comes
/// from a topological sort of the *whole* circuit, so two independent
/// gates inside the region can swap places merely because something
/// outside drives one of its inputs through one more gate than before.
/// The same module would then key differently in two patches, and a
/// lowering would work in one and silently not in the other. Tying two
/// of the region's inputs together outside it did the same thing.
/// Flattened alone, none of that exists to depend on.
///
/// Net numbering in a standalone flatten follows the module's own
/// `cells` and `ports` order, so it is stable across builds even though
/// every id in the library is a fresh uuid each time.
///
/// SHA-1, by way of `Uuid::new_v5`, because it is already a dependency.
/// That is enough to catch an edit and is **not** a security boundary:
/// someone who wants two different netlists with one digest can have
/// them.
///
/// `None` when the module cannot be flattened on its own — two ports
/// sharing a name is the realistic case, since inside a parent they are
/// addressed by id. Then it has no key, and so is not offered for
/// substitution at all.
fn module_digest(
    module_id: ModuleId,
    library: &ModuleLibrary,
    decl: &IntrinsicDecl,
) -> Option<Uuid> {
    const NAMESPACE: Uuid = Uuid::from_bytes([
        0x2f, 0x1a, 0x9c, 0x44, 0x6b, 0x3e, 0x5d, 0x71, 0x8a, 0x02, 0xc5, 0xe9, 0x37, 0x64, 0xb1,
        0xd8,
    ]);
    const SEPARATOR: u32 = u32::MAX;

    // Regions are not recorded inside: this *is* the recording of one,
    // and a module that declares itself would otherwise ask for its own
    // digest forever.
    let (flat, _) = flatten_impl(module_id, library, false, false).ok()?;
    let module = library.get(module_id)?;

    // Ports resolve through their names, which is how a standalone
    // flatten exposes them. Declared order, not port order, so swapping
    // two entries of the declaration changes the answer -- it changes
    // which argument a lowering receives.
    let net_of = |port: PortId| -> Option<NetId> {
        let name = &module.port(port)?.name;
        flat.inputs
            .iter()
            .chain(flat.outputs.iter())
            .find(|(n, _)| n == name)
            .map(|(_, net)| *net)
    };

    let mut canonical: Vec<u32> = Vec::with_capacity(flat.gates.len() * 3 + 16);
    for port in decl.inputs.iter().chain(decl.outputs.iter()) {
        canonical.push(net_of(*port)?);
    }
    canonical.push(SEPARATOR);
    for gate in &flat.gates {
        canonical.push(gate.a);
        canonical.push(gate.b);
        canonical.push(gate.y);
    }
    canonical.push(SEPARATOR);
    for reg in &flat.regs {
        canonical.push(u32::from(reg.init));
        canonical.push(reg.d);
        canonical.push(reg.q);
    }

    let bytes: Vec<u8> = canonical.iter().flat_map(|n| n.to_le_bytes()).collect();
    Some(Uuid::new_v5(&NAMESPACE, &bytes))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegSpec {
    pub d_slot: u32,
    pub init: bool,
}

/// One name-addressed top-level input bit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputBinding {
    pub name: String,
    pub(crate) slot: u32,
}

/// One name-addressed top-level output bit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputBinding {
    pub name: String,
    pub(crate) slot: u32,
}

/// An immutable compiled circuit, safe to share with a real-time
/// thread behind an `Arc`. I/O is generic: top-level ports become
/// name-addressed 1-bit inputs and outputs in declaration order, with
/// no assumptions about what the circuit computes. Register `q` slots
/// occupy `0..regs.len()`; evaluation state lives in
/// [`crate::eval::CircuitState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledCircuit {
    pub(crate) program_id: Uuid,
    pub(crate) ticks_per_step: u8,
    pub(crate) slot_count: u32,
    pub(crate) instrs: Box<[Instr]>,
    /// Regions an `Instr::Region` selects. Empty unless some module in
    /// the tree declared itself substitutable.
    pub(crate) regions: Box<[CompiledRegion]>,
    pub(crate) regs: Box<[RegSpec]>,
    pub(crate) banks: Box<[BankSpec]>,
    pub(crate) inputs: Box<[InputBinding]>,
    pub(crate) outputs: Box<[OutputBinding]>,
}

/// A recognised shift chain, evaluated as a ring buffer.
///
/// The registers it replaces are gone from `CompiledCircuit::regs`;
/// the bank holds their state and publishes the values anything still
/// reads. See [`crate::delay`] for how a chain is recognised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankSpec {
    /// Ticks of delay, and ring entries.
    pub(crate) len: u32,
    /// Shared reset value of every register the bank replaced.
    pub(crate) init: bool,
    /// Where the value entering the line is read from.
    pub(crate) ingress_slot: u32,
    /// `(distance in ticks, slot)`, distances in `1..=len`.
    pub(crate) taps: Box<[(u32, u32)]>,
}

impl CompiledCircuit {
    /// The intrinsic regions this circuit offers for substitution.
    /// Empty unless some module declared itself, and a declaration that
    /// cannot be taken whole (state, a pass-through output, no gates)
    /// does not appear here even though `flatten` still reports it.
    pub fn regions(&self) -> &[CompiledRegion] {
        &self.regions
    }

    /// Top-level instructions, counting a region as one.
    pub fn instr_count(&self) -> usize {
        self.instrs.len()
    }

    pub fn compile(asset: &CircuitAsset) -> Result<Self, CompileError> {
        if asset.ticks_per_step == 0 || asset.ticks_per_step > MAX_TICKS_PER_STEP {
            return Err(CompileError::InvalidTicksPerStep(asset.ticks_per_step));
        }
        let flat = flatten(asset.root, &asset.library)?;
        Self::from_flat(&flat, asset.ticks_per_step)
    }

    /// Compiles an already-flattened circuit with generic name-based
    /// I/O bindings. Top-level port names must be unique within each
    /// direction so that name addressing is unambiguous.
    pub fn from_flat(flat: &FlatCircuit, ticks_per_step: u8) -> Result<Self, CompileError> {
        if ticks_per_step == 0 || ticks_per_step > MAX_TICKS_PER_STEP {
            return Err(CompileError::InvalidTicksPerStep(ticks_per_step));
        }

        // Shift chains become ring buffers. This has to happen before
        // slots are assigned: the evaluator copies register state into
        // the head of the signal array and so requires the registers
        // that survive to occupy exactly the first slots. Eliding them
        // afterwards would punch holes in that range.
        let recognised = crate::delay::recognise(flat);

        // The budget that actually bounds the engine, now that the real
        // figure is known. Lowering removes a shift chain's per-tick
        // copying, not its storage, so a bank is charged its full length.
        let surviving = recognised
            .claimed
            .iter()
            .filter(|claimed| !**claimed)
            .count();
        let banked: usize = recognised.banks.iter().map(|bank| bank.len as usize).sum();
        if surviving + banked > MAX_RUNTIME_STATE {
            return Err(CompileError::TooMuchState(surviving + banked));
        }

        // Slot layout: surviving reg q slots first (contiguous copy from
        // register state each tick), then every remaining net in id
        // order — which is where a bank's taps and ingress land.
        let mut slot_of_net: Vec<Option<u32>> = vec![None; flat.net_count as usize];
        let mut next_slot: u32 = 0;
        for (index, reg) in flat.regs.iter().enumerate() {
            if recognised.claimed[index] {
                continue;
            }
            slot_of_net[reg.q as usize] = Some(next_slot);
            next_slot += 1;
        }
        for net in 0..flat.net_count {
            if slot_of_net[net as usize].is_none() {
                slot_of_net[net as usize] = Some(next_slot);
                next_slot += 1;
            }
        }
        let slot = |net: NetId| slot_of_net[net as usize].expect("all nets have slots");

        let mut seen_inputs: BTreeSet<&str> = BTreeSet::new();
        let mut inputs: Vec<InputBinding> = Vec::with_capacity(flat.inputs.len());
        for (name, net) in &flat.inputs {
            if !seen_inputs.insert(name.as_str()) {
                return Err(CompileError::DuplicateTopPort { name: name.clone() });
            }
            inputs.push(InputBinding {
                name: name.clone(),
                slot: slot(*net),
            });
        }

        let mut seen_outputs: BTreeSet<&str> = BTreeSet::new();
        let mut outputs: Vec<OutputBinding> = Vec::with_capacity(flat.outputs.len());
        for (name, net) in &flat.outputs {
            if !seen_outputs.insert(name.as_str()) {
                return Err(CompileError::DuplicateTopPort { name: name.clone() });
            }
            outputs.push(OutputBinding {
                name: name.clone(),
                slot: slot(*net),
            });
        }

        let folded = crate::fold::fold_netlist(flat, &recognised.claimed);
        let to_instr = |op: &crate::fold::FlatOp| -> Instr {
            match *op {
                crate::fold::FlatOp::Nand(gate) => Instr::Nand {
                    a: slot(gate.a),
                    b: slot(gate.b),
                    out: slot(gate.y),
                },
                crate::fold::FlatOp::FullAdder(adder) => Instr::FullAdder {
                    a: slot(adder.a),
                    b: slot(adder.b),
                    cin: slot(adder.cin),
                    sum: slot(adder.sum),
                    cout: slot(adder.cout),
                },
                crate::fold::FlatOp::Xor(xor) => Instr::Xor {
                    a: slot(xor.a),
                    b: slot(xor.b),
                    out: slot(xor.out),
                },
                crate::fold::FlatOp::And(and) => Instr::And {
                    a: slot(and.a),
                    b: slot(and.b),
                    out: slot(and.out),
                },
                crate::fold::FlatOp::Not(not) => Instr::Not {
                    a: slot(not.a),
                    out: slot(not.out),
                },
                crate::fold::FlatOp::Region(region) => Instr::Region { region },
            }
        };
        let regions: Box<[CompiledRegion]> = folded
            .regions
            .iter()
            .map(|region| {
                let decl = &flat.intrinsics()[region.region as usize];
                let inputs: Box<[u32]> = region.inputs.iter().map(|net| slot(*net)).collect();
                let outputs: Box<[u32]> = region.outputs.iter().map(|net| slot(*net)).collect();
                let fallback: Box<[Instr]> = region.fallback.iter().map(to_instr).collect();
                // The registers this region owns, in flatten order. A
                // banked one cannot appear: a region holding one is not
                // offered at all.
                let state: Box<[StateBinding]> = flat
                    .regs()
                    .iter()
                    .enumerate()
                    .filter(|(index, reg)| {
                        reg.region == Some(region.region)
                            && reg.d != reg.q
                            && !recognised.claimed[*index]
                    })
                    .map(|(_, reg)| StateBinding {
                        q_slot: slot(reg.q),
                        d_slot: slot(reg.d),
                        init: reg.init,
                    })
                    .collect();
                // Whether this region's state is nobody else's business.
                //
                // A register whose `q` is read only by the region that
                // drives it is one nothing outside can observe, so the
                // region may hold it in whatever representation suits
                // it -- lane-major, for a backend that computes in
                // lanes -- instead of round-tripping it through the
                // bit-planed register array every tick. Keeping private
                // state in lane-major form avoids that transpose cost.
                //
                // "Read only by the region" has to mean every way a net
                // can be read: a gate outside it, another register's
                // `d`, another region's declared input, a top-level
                // output, and a delay bank's ingress or tap. Missing
                // any one of those would let a value be observed in one
                // representation and written in another.
                let state_is_private = !state.is_empty()
                    && flat.regs().iter().enumerate().all(|(index, reg)| {
                        if reg.region != Some(region.region)
                            || reg.d == reg.q
                            || recognised.claimed[index]
                        {
                            return true;
                        }
                        let q = reg.q;
                        let read_by_outside_gate = flat.gates().iter().any(|gate| {
                            (gate.a == q || gate.b == q) && gate.region != Some(region.region)
                        });
                        let feeds_another_register = flat
                            .regs()
                            .iter()
                            .any(|other| other.d == q && other.region != Some(region.region));
                        let is_top_output = flat.outputs.iter().any(|(_, net)| *net == q);
                        let feeds_another_region =
                            flat.intrinsics().iter().enumerate().any(|(other, decl)| {
                                other != region.region as usize && decl.inputs.contains(&q)
                            });
                        !read_by_outside_gate
                            && !feeds_another_register
                            && !is_top_output
                            && !feeds_another_region
                    });
                // And every one of them must start at zero.
                //
                // The buffer the backend keeps starts zeroed, and the
                // engine cannot initialise a layout it does not know --
                // only the lowering knows what its words mean. So the
                // contract is that all-zero is the state the registers'
                // `init` describes, and the only way to guarantee that
                // is to require the inits to be zero.
                let state_is_private = state_is_private
                    && flat.regs().iter().all(|reg| {
                        reg.region != Some(region.region) || reg.d == reg.q || !reg.init
                    });
                CompiledRegion {
                    name: decl.name.clone(),
                    revision: decl.revision,
                    digest: decl.digest.expect("only keyed regions are offered"),
                    inputs,
                    outputs,
                    state,
                    state_is_private,
                    fallback,
                }
            })
            .collect();

        Ok(Self {
            program_id: Uuid::new_v4(),
            ticks_per_step,
            slot_count: next_slot,
            instrs: folded.ops.iter().map(to_instr).collect(),
            regions,
            banks: recognised
                .banks
                .iter()
                .map(|bank| BankSpec {
                    len: bank.len,
                    init: bank.init,
                    ingress_slot: slot(bank.ingress),
                    taps: bank
                        .taps
                        .iter()
                        .map(|(distance, net)| (*distance, slot(*net)))
                        .collect(),
                })
                .collect(),
            regs: flat
                .regs
                .iter()
                .enumerate()
                .filter(|(index, _)| !recognised.claimed[*index])
                .map(|(_, reg)| RegSpec {
                    d_slot: slot(reg.d),
                    init: reg.init,
                })
                .collect(),
            inputs: inputs.into(),
            outputs: outputs.into(),
        })
    }

    /// The index of a top-level input by port name.
    pub fn input_index(&self, name: &str) -> Option<usize> {
        self.inputs.iter().position(|binding| binding.name == name)
    }

    /// The index of a top-level output by port name.
    pub fn output_index(&self, name: &str) -> Option<usize> {
        self.outputs.iter().position(|binding| binding.name == name)
    }

    pub fn input_count(&self) -> usize {
        self.inputs.len()
    }

    pub fn output_count(&self) -> usize {
        self.outputs.len()
    }

    /// Top-level input names, in declaration order.
    pub fn input_names(&self) -> impl Iterator<Item = &str> {
        self.inputs.iter().map(|binding| binding.name.as_str())
    }

    /// Top-level output names, in declaration order.
    pub fn output_names(&self) -> impl Iterator<Item = &str> {
        self.outputs.iter().map(|binding| binding.name.as_str())
    }

    pub fn ticks_per_step(&self) -> u8 {
        self.ticks_per_step
    }

    /// Number of compiled instructions (NANDs plus fused adders).
    pub fn gate_count(&self) -> usize {
        self.instrs.len()
    }

    /// Number of full adders recognised and fused by the folding pass.
    pub fn folded_adder_count(&self) -> usize {
        self.instrs
            .iter()
            .filter(|instr| matches!(instr, Instr::FullAdder { .. }))
            .count()
    }

    /// Number of standalone XORs recognised and fused by the folding
    /// pass (XORs inside fused adders are not counted).
    pub fn folded_xor_count(&self) -> usize {
        self.instrs
            .iter()
            .filter(|instr| matches!(instr, Instr::Xor { .. }))
            .count()
    }

    pub fn reg_count(&self) -> usize {
        self.regs.len()
    }

    /// Number of shift chains lowered to delay banks ([`crate::delay`]).
    pub fn bank_count(&self) -> usize {
        self.banks.len()
    }

    /// Words of state the compiled circuit owns: one per surviving
    /// register, plus every bank's whole ring. This is the figure
    /// [`MAX_RUNTIME_STATE`] bounds — lowering a chain removes its
    /// per-tick copying, not its storage — so it is the honest answer to
    /// "how big is this patch", where [`Self::reg_count`] alone is not.
    pub fn state_words(&self) -> usize {
        self.regs.len()
            + self
                .banks
                .iter()
                .map(|bank| bank.len as usize)
                .sum::<usize>()
    }
}

#[cfg(test)]
mod combinational_deps_tests {
    use super::combinational_deps;
    use crate::stdlib::Stdlib;

    /// A gate passes through, on both operands.
    #[test]
    fn a_gate_depends_on_both_its_inputs() {
        let stdlib = Stdlib::build();
        let deps = combinational_deps(stdlib.xor_gate, &stdlib.library).expect("flattens");
        assert!(deps.depends("y", "a"));
        assert!(deps.depends("y", "b"));
    }

    /// A register breaks the tick: the LFSR's output is last
    /// tick's, so nothing upstream of it is a same-tick dependency. This
    /// is the property that makes a feedback loop legal, and it is now
    /// **read off the netlist** rather than asserted by naming a module.
    #[test]
    fn a_register_breaks_the_dependency() {
        let stdlib = Stdlib::build();
        let deps = combinational_deps(stdlib.lfsr16, &stdlib.library).expect("flattens");
        for (output, inputs) in &deps.outputs {
            assert!(
                inputs.is_empty(),
                "{output} is latched, so it owes this tick nothing; got {inputs:?}"
            );
        }
    }

    /// The case a per-module answer can never give: **one** module, both
    /// answers. `now` follows `a` within the tick; `later` is `a` through
    /// a register and owes this tick nothing. No property of the module
    /// as a whole is true of one and false of the other, so asking per
    /// signal is not a refinement here — it is the only question with an
    /// answer.
    ///
    /// The two other shapes worth pinning are here too: `straight` is an
    /// input wired directly to an output, with no gate in between, and
    /// `now` reaches `a` down two paths of different lengths that
    /// reconverge — neither may confuse the walk.
    #[test]
    fn state_is_a_property_of_a_signal_not_of_a_block() {
        use crate::model::{Endpoint, ModuleBuilder, ModuleLibrary};

        let mut b = ModuleBuilder::new_canonical("test/both-answers");
        let a = b.input("a");
        let straight = b.output("straight");
        let now = b.output("now");
        let later = b.output("later");

        b.wire(Endpoint::module(a), Endpoint::module(straight));

        // now = nand(a, not a): the same source down a one-gate path and
        // a two-gate path, rejoining.
        let inv = b.not_gate(Endpoint::module(a));
        let join = b.nand();
        b.wire(Endpoint::module(a), Endpoint::nand_a(join));
        b.wire(Endpoint::nand_y(inv), Endpoint::nand_b(join));
        b.wire(Endpoint::nand_y(join), Endpoint::module(now));

        let held = b.reg(false);
        b.wire(Endpoint::module(a), Endpoint::reg_d(held));
        b.wire(Endpoint::reg_q(held), Endpoint::module(later));

        let module = b.finish();
        let root = module.id;
        let mut library = ModuleLibrary::default();
        library.insert(module);

        let deps = combinational_deps(root, &library).expect("flattens");
        assert!(deps.depends("straight", "a"), "a bare wire is a dependency");
        assert!(
            deps.depends("now", "a"),
            "reconvergent paths still reach `a`"
        );
        assert!(
            !deps.depends("later", "a"),
            "the register breaks the tick, on this output only"
        );
        assert_eq!(
            deps.outputs.len(),
            3,
            "each output is reported exactly once: {:?}",
            deps.outputs
        );
    }
}
/// What a backend will actually execute, counted.
///
/// **Not a cost.** A cost is nanoseconds, nanoseconds are a property of
/// a machine, and this crate knows nothing about machines, clock rates
/// or real-time policy. What it can say is *how much of what* runs, and
/// leave the arithmetic to whoever knows what the work costs on the
/// hardware in front of them.
///
/// The distinction earns its keep the moment a region is substituted. A
/// gate count says a lowered circuit is as expensive as its gates, which
/// is exactly wrong: the gates are the one thing that does not run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutionManifest {
    /// Two-input NANDs.
    pub nands: usize,
    /// Fused full adders, each five bitwise operations.
    pub full_adders: usize,
    /// Fused XORs.
    pub xors: usize,
    /// Fused ANDs.
    pub ands: usize,
    /// Inverters.
    pub nots: usize,
    /// Regions replaced by native code, and what crossed their
    /// boundaries.
    pub substitutions: Vec<SubstitutionCost>,
    /// Registers latched per tick.
    pub state_words: usize,
    /// Ring entries across every lowered delay line. Storage, not work,
    /// but it is memory traffic and a caller costing a circuit will want
    /// it.
    pub bank_words: usize,
    /// The longest chain of dependent operations, weighted the same way
    /// `bitwise_ops` weighs them.
    ///
    /// Two circuits with the same amount of work can take very
    /// different times: one the processor can run wide, and one where
    /// every operation waits for the last. Counting only the work says
    /// they cost the same, and on the circuits here they differ by 2.4
    /// times.
    pub span_ops: usize,
    /// How many times the whole of this runs per step.
    pub ticks_per_step: u8,
}

/// How long a substitution counts as, in the same weighted operations
/// `span_ops` uses. A transpose pair and a call are one long dependent
/// step whatever the arithmetic between them was.
const SUBSTITUTION_SPAN: usize = 600;

/// The longest dependent chain through a run of instructions.
///
/// **In execution order, regions included where they sit.** An
/// unsubstituted region runs its fallback exactly where the region node
/// is — `eval.rs` does it, and the JIT emits it inline — so the walk
/// has to descend there too.
///
/// It did not, at first: the top level skipped unsubstituted regions
/// and the fallbacks were walked afterwards in a second pass. Anything
/// downstream had already read the region's outputs at depth zero, so
/// every chain running *through* a region was cut short and the span
/// came out too small. Too small is the dangerous direction — a cost
/// model that underpredicts admits a circuit that then misses its
/// deadline.
///
/// The recursion is one level deep by construction: flattening claims
/// nested declarations into the outermost, so a fallback never contains
/// a region.
fn walk_span(
    instrs: &[Instr],
    depth: &mut Vec<usize>,
    span: &mut usize,
    regions: &[CompiledRegion],
    substituted: &[u32],
) {
    for instr in instrs {
        let (reads, writes, weight): (Vec<u32>, Vec<u32>, usize) = match *instr {
            Instr::Nand { a, b, out } => (vec![a, b], vec![out], 2),
            Instr::FullAdder {
                a,
                b,
                cin,
                sum,
                cout,
            } => (vec![a, b, cin], vec![sum, cout], 5),
            Instr::Xor { a, b, out } => (vec![a, b], vec![out], 1),
            Instr::And { a, b, out } => (vec![a, b], vec![out], 1),
            Instr::Not { a, out } => (vec![a], vec![out], 1),
            Instr::Region { region } => {
                let compiled = &regions[region as usize];
                if substituted.contains(&region) {
                    // A substitution is one long dependent step:
                    // everything in it waits for the transpose and the
                    // call.
                    (
                        compiled.inputs.to_vec(),
                        compiled.outputs.to_vec(),
                        SUBSTITUTION_SPAN,
                    )
                } else {
                    walk_span(&compiled.fallback, depth, span, regions, substituted);
                    continue;
                }
            }
        };
        let arrival = reads
            .iter()
            .map(|slot| depth[*slot as usize])
            .max()
            .unwrap_or(0)
            + weight;
        for slot in writes {
            let at = &mut depth[slot as usize];
            *at = (*at).max(arrival);
        }
        *span = (*span).max(arrival);
    }
}

/// What one instruction lowers to, in bitwise operations.
///
/// The same weights [`ExecutionManifest::bitwise_ops`] uses, so a
/// region's fallback can be priced in the currency everything else is
/// priced in.
fn op_weight(instr: &Instr) -> usize {
    match instr {
        Instr::Nand { .. } => 2,
        Instr::FullAdder { .. } => 5,
        Instr::Xor { .. } | Instr::And { .. } | Instr::Not { .. } => 1,
        Instr::Region { .. } => 0,
    }
}

/// One region a backend replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubstitutionCost {
    /// The key the lowering was registered under.
    pub name: String,
    /// Bit planes handed to it.
    pub inputs: usize,
    /// Bit planes handed back.
    pub outputs: usize,
    /// Bitwise operations its gates would have taken -- what the
    /// substitution removed, in the same currency `bitwise_ops` counts.
    ///
    /// Against the boundary width, this is what says whether a
    /// substitution is worth making at all: a region with cheap gates
    /// and a wide boundary costs *more* substituted, because a
    /// transpose is priced by what crosses it.
    pub replaced_ops: usize,
    /// Instructions its gates would have taken, which is what the
    /// substitution removed.
    pub replaced: usize,
}

impl ExecutionManifest {
    /// Instructions that run, counting a substitution as none of them.
    pub fn instruction_count(&self) -> usize {
        self.nands + self.full_adders + self.xors + self.ands + self.nots
    }
}

impl CompiledCircuit {
    /// What an evaluator with no native lowerings will run: every
    /// instruction, and every region's gates.
    ///
    /// This is the reference interpreter's manifest, and the JIT's too
    /// when nothing matched.
    pub fn manifest(&self) -> ExecutionManifest {
        self.manifest_with(&[])
    }

    /// The same, for a backend that substituted the given regions.
    ///
    /// The indices must be the ones a program *actually* substituted
    /// (`crate::jit::JitProgram::substituted_regions`), never the ones a
    /// registry happens to hold: a key that does not match is a
    /// lowering that exists and was not used.
    pub fn manifest_with(&self, substituted: &[u32]) -> ExecutionManifest {
        let mut manifest = ExecutionManifest {
            state_words: self.regs.len(),
            bank_words: self.banks.iter().map(|bank| bank.len as usize).sum(),
            ticks_per_step: self.ticks_per_step,
            ..ExecutionManifest::default()
        };
        let count = |instrs: &[Instr], manifest: &mut ExecutionManifest| {
            for instr in instrs {
                match instr {
                    Instr::Nand { .. } => manifest.nands += 1,
                    Instr::FullAdder { .. } => manifest.full_adders += 1,
                    Instr::Xor { .. } => manifest.xors += 1,
                    Instr::And { .. } => manifest.ands += 1,
                    Instr::Not { .. } => manifest.nots += 1,
                    Instr::Region { .. } => {}
                }
            }
        };
        count(&self.instrs, &mut manifest);
        for (index, region) in self.regions.iter().enumerate() {
            if substituted.contains(&(index as u32)) {
                manifest.substitutions.push(SubstitutionCost {
                    name: region.name.clone(),
                    inputs: region.inputs.len(),
                    outputs: region.outputs.len(),
                    replaced: region.fallback.len(),
                    replaced_ops: region.fallback.iter().map(op_weight).sum(),
                });
            } else {
                count(&region.fallback, &mut manifest);
            }
        }
        // The longest dependent chain, walked in the order the backend
        // will emit. Instructions are topologically sorted, so a slot's
        // depth is settled before anything reads it; register `q` slots
        // and top-level inputs start at zero because they are loaded
        // rather than computed.
        let mut depth = vec![0_usize; self.slot_count as usize];
        let mut span = 0_usize;
        walk_span(
            &self.instrs,
            &mut depth,
            &mut span,
            &self.regions,
            substituted,
        );
        manifest.span_ops = span;

        manifest
    }
}

impl ExecutionManifest {
    /// Primitive bitwise operations these instructions become.
    ///
    /// A backend does not execute instructions, it executes the
    /// operations they lower to, and the classes differ: a NAND is an
    /// `and` and a `not`, a fused full adder is the five-operation
    /// generate/propagate form, and an XOR, AND or inverter is one.
    /// Counting instructions treats a full adder as an inverter, which
    /// is off by five to one.
    ///
    /// These weights are this engine's own lowering
    /// (`crate::eval`, `crate::jit`), so they belong here rather than
    /// with whoever is costing the work.
    pub fn bitwise_ops(&self) -> usize {
        self.nands * 2 + self.full_adders * 5 + self.xors + self.ands + self.nots
    }
}
