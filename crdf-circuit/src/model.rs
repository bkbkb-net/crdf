//! Editable circuit model: modules, ports, cells and wires.
//!
//! The model is the persisted shape (see `crdf_io`). It is deliberately
//! tiny: the only computational primitives are the two axiom boxes
//! `nand` (ports `a`, `b` → `y`) and `reg` (port `d` → `q`, one-tick
//! delay with an initial bit). Everything else is structure: module
//! hierarchy that flattens away at compile time, and 1-bit ports used
//! for composition. Constants need no primitive: a `reg` whose `q` is
//! wired back to its own `d` holds its `init` bit forever.

use std::collections::BTreeMap;

use uuid::Uuid;

/// Hard cap on flattened NAND gates in one compiled circuit.
pub const MAX_FLATTENED_GATES: usize = 65_536;

/// Hard cap on registers the compiler will **materialise**.
///
/// This is a *compile* budget — how big a netlist the compiler is willing
/// to build and how long it may take — and is deliberately not the same
/// question as how much state a circuit may own at runtime, which is
/// [`MAX_RUNTIME_STATE`]. The two were one number, which meant a delay
/// line long enough to be useful was rejected while being flattened,
/// before the pass that would have collapsed it into a ring ever saw it.
///
/// Chosen from measurement rather than roundness: flattening and
/// compiling is linear at roughly one millisecond per thousand
/// registers (16k in 16 ms, 64k in 67 ms, release build), so this admits
/// about a million registers for about a second of compile. Compiling is
/// expected to happen off any realtime thread.
pub const MAX_FLATTENED_REGS: usize = 1_048_576;

/// Hard cap on the state a compiled circuit owns at runtime, counted in
/// 64-lane words: ordinary registers plus every delay bank's ring.
///
/// Checked *after* lowering, because that is when the real figure is
/// known. Lowering removes the per-tick copying of a shift chain, not
/// its storage, so a bank is charged its full length.
pub const MAX_RUNTIME_STATE: usize = 1_048_576;
/// Hard cap on module instantiation depth (root = depth 0).
pub const MAX_HIERARCHY_DEPTH: usize = 16;
/// Valid range for [`CircuitAsset::ticks_per_step`] is `1..=MAX`.
pub const MAX_TICKS_PER_STEP: u8 = 64;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new_random() -> Self {
                Self(Uuid::new_v4())
            }
        }
    };
}

id_type!(
    /// Identity of a module (a reusable sub-circuit definition).
    ModuleId
);
id_type!(
    /// Identity of a port (a 1-bit composition boundary of a module).
    PortId
);
id_type!(
    /// Identity of a cell (an occurrence of an axiom box or a module).
    CellId
);
id_type!(
    /// Identity of a wire (one immutable point-to-point connection).
    WireId
);
id_type!(
    /// Identity of a playable asset (root module + tick rate). An
    /// asset's identity is independent of its root module, so several
    /// assets may share one root (for example at different tick
    /// rates) inside the same graph.
    AssetId
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDirection {
    Input,
    Output,
}

/// A 1-bit connection point on a module boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub id: PortId,
    pub name: String,
    pub direction: PortDirection,
}

/// What a cell is an occurrence of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    /// Axiom box: `y = !(a & b)`, evaluated with zero delay.
    Nand,
    /// Axiom box: one-tick delay. `q` outputs the previous tick's `d`;
    /// on lane reset `q` outputs `init`.
    Reg { init: bool },
    /// An occurrence of another module, flattened away at compile time.
    Instance { module: ModuleId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub id: CellId,
    pub kind: CellKind,
}

/// A pin of a cell: the well-known axiom-box pins, or a port of the
/// instantiated module for [`CellKind::Instance`] cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortRef {
    NandA,
    NandB,
    NandY,
    RegD,
    RegQ,
    Inner(PortId),
}

/// One end of a wire, inside a given module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// A pin of a cell in this module.
    Cell { cell: CellId, port: PortRef },
    /// A port of this module itself (the "outer box").
    Module { port: PortId },
}

/// One immutable point-to-point connection. `from` must be a signal
/// source (NAND `y`, reg `q`, instance output, or an input port of the
/// enclosing module); `to` must be a signal sink.
///
/// CRDT contract: a wire's endpoints are fixed for its lifetime.
/// Editors change a connection by deleting the wire and creating a new
/// one under a fresh id — never by mutating `from`/`to` in place —
/// so concurrent merges cannot produce half-rewired connections. The
/// fields stay public for construction and tests; the invariant is
/// enforced at the editing/CRDT layer, not by this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wire {
    pub id: WireId,
    pub from: Endpoint,
    pub to: Endpoint,
}

/// A module's claim that a compiler may replace the whole of it with
/// something other than its gates.
///
/// **This is a claim about equality, not a hint.** The gates stay
/// authoritative: they are what the plain evaluator runs, and any
/// substitute a backend supplies has to agree with them bit for bit, on
/// every lane, at every input. A backend that has nothing registered
/// under the name simply evaluates the gates, so declaring an intrinsic
/// never changes what a circuit computes — only, possibly, how fast.
///
/// `revision` exists so a netlist can be edited without silently
/// keeping a substitute that no longer matches it. A backend registers
/// its lowering against a particular revision; bumping the number
/// retires every registration until someone re-checks them.
///
/// Nothing here says what the computation *is*. The engine carries the
/// name from the module to the backend and takes no view on it; which
/// names mean anything is entirely the backend's business.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntrinsicDecl {
    /// An opaque key. The engine compares it and passes it on.
    pub name: String,
    /// Bumped whenever the gates change in a way a substitute would
    /// have to follow.
    pub revision: u32,
    /// The module's own ports, in the order a substitute receives them.
    /// Declaration order is not assumed: a substitute's first argument
    /// is whatever this lists first.
    pub inputs: Vec<PortId>,
    /// Likewise for results.
    pub outputs: Vec<PortId>,
}

/// A reusable sub-circuit definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub id: ModuleId,
    pub name: String,
    pub ports: Vec<Port>,
    pub cells: Vec<Cell>,
    pub wires: Vec<Wire>,
    /// **How** this module was built, when its author said so
    /// (`crate::structure`).
    ///
    /// Provenance, never semantics: no evaluator reads it, flattening
    /// ignores it, and a module with none is not different — only
    /// undescribed. It lives *here*, on the module, because a term kept
    /// in a side table beside one library is not retained at all: it
    /// cannot travel with the module, cannot be attached by any builder
    /// but that library's own, and vanishes the moment anything reloads.
    pub structures: Vec<crate::structure::Structure>,
    /// **What** this module is, when a backend might have a faster way
    /// to compute it than running its gates ([`IntrinsicDecl`]).
    ///
    /// Unlike `structures`, this one is load-bearing: a compiler may act
    /// on it. It is still not semantics — the gates remain the
    /// definition, and the declaration only asserts that something else
    /// may be equal to them.
    pub intrinsic: Option<IntrinsicDecl>,
}

impl Module {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: ModuleId::new_random(),
            name: name.into(),
            ports: Vec::new(),
            cells: Vec::new(),
            wires: Vec::new(),
            structures: Vec::new(),
            intrinsic: None,
        }
    }

    pub fn port(&self, id: PortId) -> Option<&Port> {
        self.ports.iter().find(|p| p.id == id)
    }

    pub fn cell(&self, id: CellId) -> Option<&Cell> {
        self.cells.iter().find(|c| c.id == id)
    }
}

/// A set of module definitions, keyed and iterated in deterministic
/// (UUID) order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleLibrary {
    modules: BTreeMap<ModuleId, Module>,
}

impl ModuleLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a module definition, replacing any previous definition
    /// with the same id.
    pub fn insert(&mut self, module: Module) {
        self.modules.insert(module.id, module);
    }

    pub fn get(&self, id: ModuleId) -> Option<&Module> {
        self.modules.get(&id)
    }

    pub fn modules(&self) -> impl Iterator<Item = &Module> {
        self.modules.values()
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

/// A loadable / playable circuit: a root module, the tick rate it is
/// meant to run at, and the transitive closure of module definitions it
/// needs. `ticks_per_step` lives here — not on [`Module`] — so the
/// same module can be instantiated as a top at different rates, and
/// the asset's own [`AssetId`] keeps such variants distinct within one
/// graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CircuitAsset {
    pub id: AssetId,
    pub root: ModuleId,
    pub ticks_per_step: u8,
    pub library: ModuleLibrary,
}

impl CircuitAsset {
    /// A new asset with a fresh identity.
    pub fn new(root: ModuleId, ticks_per_step: u8, library: ModuleLibrary) -> Self {
        Self {
            id: AssetId::new_random(),
            root,
            ticks_per_step,
            library,
        }
    }
}

/// Convenience constructors for wire endpoints.
impl Endpoint {
    pub const fn nand_a(cell: CellId) -> Self {
        Self::Cell {
            cell,
            port: PortRef::NandA,
        }
    }

    pub const fn nand_b(cell: CellId) -> Self {
        Self::Cell {
            cell,
            port: PortRef::NandB,
        }
    }

    pub const fn nand_y(cell: CellId) -> Self {
        Self::Cell {
            cell,
            port: PortRef::NandY,
        }
    }

    pub const fn reg_d(cell: CellId) -> Self {
        Self::Cell {
            cell,
            port: PortRef::RegD,
        }
    }

    pub const fn reg_q(cell: CellId) -> Self {
        Self::Cell {
            cell,
            port: PortRef::RegQ,
        }
    }

    pub const fn inner(cell: CellId, port: PortId) -> Self {
        Self::Cell {
            cell,
            port: PortRef::Inner(port),
        }
    }

    pub const fn module(port: PortId) -> Self {
        Self::Module { port }
    }
}

/// Namespace UUID for canonical (UUIDv5-derived) entity identities.
/// Fixed forever: changing it would re-identify every canonical
/// module.
pub const CANONICAL_NAMESPACE: Uuid = Uuid::from_u128(0x6c1b_8f0a_3e42_4c65_a1ef_dd0a_5c1b_c17c);

/// UUIDv5 of a name path under [`CANONICAL_NAMESPACE`].
pub fn canonical_uuid(name: &str) -> Uuid {
    Uuid::new_v5(&CANONICAL_NAMESPACE, name.as_bytes())
}

/// Incremental builder used by the standard library and tests.
///
/// [`ModuleBuilder::new`] assigns fresh random identities (right for
/// user circuits: every occurrence is a distinct resource).
/// [`ModuleBuilder::new_canonical`] instead derives every identity as
/// a UUIDv5 of a stable name path under [`CANONICAL_NAMESPACE`] —
/// `<module name>` for the module, `<module name>/port/<port name>`
/// for ports and `<module name>/{cell,wire}/<ordinal>` (construction
/// order) for cells and wires. Two canonical builds of the same code
/// therefore produce byte-identical libraries, so shared library
/// modules deduplicate across assets, files and processes. Standard
/// library modules use this; note that structurally changing a
/// canonical module's build code changes its ordinals and hence its
/// identity — which is the intended "new version = new entity"
/// behaviour.
#[derive(Debug)]
pub struct ModuleBuilder {
    module: Module,
    /// Base name path for canonical identity derivation.
    canonical: Option<String>,
}

impl ModuleBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            module: Module::new(name),
            canonical: None,
        }
    }

    /// A builder whose module and members receive deterministic,
    /// name-derived identities (see the type docs).
    pub fn new_canonical(name: impl Into<String>) -> Self {
        let name = name.into();
        let mut module = Module::new(name.clone());
        module.id = ModuleId(canonical_uuid(&name));
        Self {
            module,
            canonical: Some(name),
        }
    }

    fn canonical_id(&self, suffix: &str) -> Option<Uuid> {
        self.canonical
            .as_ref()
            .map(|base| canonical_uuid(&format!("{base}/{suffix}")))
    }

    pub fn id(&self) -> ModuleId {
        self.module.id
    }

    pub fn input(&mut self, name: impl Into<String>) -> PortId {
        self.port(name, PortDirection::Input)
    }

    pub fn output(&mut self, name: impl Into<String>) -> PortId {
        self.port(name, PortDirection::Output)
    }

    fn port(&mut self, name: impl Into<String>, direction: PortDirection) -> PortId {
        let name = name.into();
        let id = match self.canonical_id(&format!("port/{name}")) {
            Some(uuid) => {
                debug_assert!(
                    self.module.ports.iter().all(|port| port.name != name),
                    "canonical modules require unique port names"
                );
                PortId(uuid)
            }
            None => PortId::new_random(),
        };
        self.module.ports.push(Port {
            id,
            name,
            direction,
        });
        id
    }

    pub fn nand(&mut self) -> CellId {
        self.cell(CellKind::Nand)
    }

    pub fn reg(&mut self, init: bool) -> CellId {
        self.cell(CellKind::Reg { init })
    }

    pub fn instance(&mut self, module: ModuleId) -> CellId {
        self.cell(CellKind::Instance { module })
    }

    /// Adds one cell of any kind, which is what a structural term over
    /// primitives needs: a bank of sixteen registers is as real a
    /// repetition as sixteen instances of a module, and `Lane::body`
    /// covers both.
    pub fn cell(&mut self, kind: CellKind) -> CellId {
        let ordinal = self.module.cells.len();
        let id = match self.canonical_id(&format!("cell/{ordinal}")) {
            Some(uuid) => CellId(uuid),
            None => CellId::new_random(),
        };
        self.module.cells.push(Cell { id, kind });
        id
    }

    pub fn wire(&mut self, from: Endpoint, to: Endpoint) -> WireId {
        let ordinal = self.module.wires.len();
        let id = match self.canonical_id(&format!("wire/{ordinal}")) {
            Some(uuid) => WireId(uuid),
            None => WireId::new_random(),
        };
        self.module.wires.push(Wire { id, from, to });
        id
    }

    /// Adds a NAND wired as an inverter fed by `source`; returns its `y`
    /// endpoint's cell.
    pub fn not_gate(&mut self, source: Endpoint) -> CellId {
        let gate = self.nand();
        self.wire(source, Endpoint::nand_a(gate));
        self.wire(source, Endpoint::nand_b(gate));
        gate
    }

    /// Adds a constant bit built from a self-holding register.
    pub fn constant(&mut self, value: bool) -> CellId {
        let reg = self.reg(value);
        self.wire(Endpoint::reg_q(reg), Endpoint::reg_d(reg));
        reg
    }

    /// Records how a repetition was built, so what the author knew
    /// survives on the module itself.
    /// What kind of cell `id` is, if the module under construction has
    /// one. Lets a structural term be checked against what was actually
    /// built before it is recorded.
    pub fn cell_kind(&self, id: CellId) -> Option<CellKind> {
        self.module
            .cells
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.kind)
    }

    pub fn record(&mut self, term: crate::structure::Structure) {
        self.module.structures.push(term);
    }

    pub fn finish(self) -> Module {
        self.module
    }
}
