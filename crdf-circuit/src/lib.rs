//! General-purpose synchronous digital circuits persisted as CRDF.
//!
//! Circuits are netlists over exactly two axiom boxes — `nand` and
//! `reg` (one-tick delay with an initial bit) — plus module hierarchy
//! that flattens away at compile time. The design follows the
//! categorical circuit literature (symmetric traced monoidal circuits
//! with delay-guarded feedback): every directed cycle must cross a
//! register, every net has exactly one driver, and evaluation is a
//! deterministic combinational pass per tick with a simultaneous
//! register latch.
//!
//! Nothing in this crate is domain-specific. Top-level I/O is generic
//! and name-addressed: inputs are levels ([`eval::CircuitState::set_level`])
//! or one-tick pulses ([`eval::CircuitState::pulse`]); outputs are read
//! back as 64-lane words. `ticks_per_step` is the only timing notion —
//! how many circuit ticks one caller step advances. Domain conventions
//! (for example a fixed port-naming ABI) are adapters layered on top by
//! consumers.
//!
//! Evaluation is bit-sliced: one compiled program advances 64
//! independent instances per instruction pass ([`eval`]), with an
//! optional Cranelift JIT backend ([`jit`], feature `jit`, on by
//! default) that compiles one tick into straight-line native code.
//! A folding pass fuses recognised full-adder and XOR patterns before
//! code generation. Persistence is CRDF ([`crdf_io`]) under the
//! `crdf:` vocabulary, with wires as immutable first-class resources
//! for CRDT-merge safety.
//!
//! The [`stdlib`] ships original textbook building blocks (gates,
//! adders, multipliers, a comparator, selectors, an LFSR), all
//! constructed from `nand`+`reg` only.

pub mod adapter;
pub mod block_dag;
pub mod compile;
pub mod connect;
pub mod crdf_io;
pub mod delay;
pub mod editor;
pub mod eval;
pub(crate) mod fold;
pub mod interp;
#[cfg(feature = "jit")]
pub mod jit;
pub mod model;
pub mod stdlib;
pub mod structure;
pub mod transpose;
pub mod types;
pub mod vocab;

pub use adapter::build_word_convert;
pub use block_dag::{BlockDag, BlockDagError, BlockInterface, BlockMetadata, BusRef, RoleTag};
pub use compile::{
    CombinationalDeps, CompileError, CompiledCircuit, FlatCircuit, HierNetMap, NetId,
    combinational_deps, flatten, flatten_with_net_map,
};
pub use connect::{
    ConnectionPlan, Mismatch, WordConvert, WordFit, bitcast_available, plan_connection,
};
pub use crdf_io::{
    CIRCUIT_CRDF_SCHEMA_VERSION, CircuitCrdfError, asset_from_rdf, asset_from_rdf_by_id,
    asset_to_rdf, list_assets, load_asset_file, load_block_dag_file, read_block_dag,
    read_block_interface, save_asset_file, save_block_dag_file, write_asset_into,
    write_asset_into_with_operations, write_block_dag_into, write_block_interface_into,
    write_module_into, write_module_into_with_operations,
};
pub use editor::CircuitEditor;
pub use eval::{CircuitState, LANES};
pub use interp::FlatSimulator;
#[cfg(feature = "jit")]
pub use jit::{JitError, JitProgram};
pub use model::{
    AssetId, Cell, CellId, CellKind, CircuitAsset, Endpoint, MAX_FLATTENED_GATES,
    MAX_FLATTENED_REGS, MAX_HIERARCHY_DEPTH, MAX_TICKS_PER_STEP, Module, ModuleBuilder, ModuleId,
    ModuleLibrary, Port, PortDirection, PortId, PortRef, Wire, WireId,
};
pub use stdlib::Stdlib;
pub use types::{Type, TypeError, Word};

/// Reading and writing a region's private lane state, in the layout the
/// engine resets.
///
/// **The layout is byte-granular.** A region with `n` registers gives
/// each lane `ceil(n / 8)` whole bytes, so lane `l` owns bytes
/// `k·l .. k·l + k` of the buffer. Nothing is packed across a byte
/// boundary, so the engine can clear one lane without knowing what the
/// bytes mean, and a helper can reach its lane with one address.
///
/// Byte order never comes into it. The engine only ever *zeroes* those
/// bytes, and zero is zero in any interpretation; what the helper reads
/// them as is between the helper and itself, since nothing else looks.
///
/// Two earlier versions of this are worth recording, because each was
/// wrong in a way the other suggested.
///
/// **Bit-granular** — lane `l` at bit `n·l` — is the tightest packing
/// and made the reset depend on how a helper read the bits, which the
/// engine cannot check. Reaching one lane took a shift and a mask, and
/// where two lanes shared a word, a read-modify-write: 1,300 ns against
/// 811 for a helper that simply cast the pointer. A contract that is
/// expensive to keep is a contract people will not keep.
///
/// **Word-granular** — a whole `u64` per lane — fixed the contract and
/// doubled the memory the loop touches for a 32-bit state, which cost
/// about as much as the bit twiddling did. The state footprint per tick
/// is what this costs, not the addressing.
pub mod lane_state {
    use uuid::Uuid;

    /// Where one region's private state sits, and what it is.
    ///
    /// The offset and length say how the buffer is carved up; the key says
    /// what the bytes mean. Two programs can carve identically and mean
    /// different things -- different regions of the same size, or the same
    /// region lowered differently -- and a state carried between them would
    /// be reading somebody else's encoding with nothing to say so.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct LaneStateBinding {
        /// First `u64` word of this region's slice.
        pub offset: usize,
        /// How many words it takes.
        pub words: usize,
        /// The region's declared name.
        pub name: String,
        /// Its declared revision.
        pub revision: u32,
        /// The digest of the gates it stands for.
        pub digest: Uuid,
    }

    /// How many bytes one lane owns.
    #[inline]
    pub const fn bytes_per_lane(registers: usize) -> usize {
        registers.div_ceil(8)
    }

    /// How many `u64` words a region with `registers` registers takes:
    /// enough to hold 64 lanes of [`bytes_per_lane`], rounded up.
    #[inline]
    pub const fn words_for(registers: usize) -> usize {
        (64 * bytes_per_lane(registers)).div_ceil(8)
    }

    /// Lane `l`'s first byte.
    ///
    /// # Safety
    /// `base` must point to at least `words_for(registers)` readable
    /// `u64`s and `lane` must be under 64.
    #[inline]
    pub unsafe fn lane(base: *const u64, registers: usize, lane: usize) -> *const u8 {
        debug_assert!(lane < 64);
        // SAFETY: the caller's guarantee.
        unsafe { base.cast::<u8>().add(bytes_per_lane(registers) * lane) }
    }

    /// The same, to write through.
    ///
    /// # Safety
    /// As [`lane`], and `base` must be writable.
    #[inline]
    pub unsafe fn lane_mut(base: *mut u64, registers: usize, lane: usize) -> *mut u8 {
        debug_assert!(lane < 64);
        // SAFETY: the caller's guarantee.
        unsafe { base.cast::<u8>().add(bytes_per_lane(registers) * lane) }
    }
}
