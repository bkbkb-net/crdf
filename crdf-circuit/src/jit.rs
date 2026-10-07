//! Cranelift JIT backend (feature `jit`, on by default).
//!
//! Compiles one tick of a [`CompiledCircuit`] into straight-line native
//! code: every net becomes an SSA value, so Cranelift's register
//! allocator keeps hot intermediates out of memory entirely — the
//! interpreter's per-instruction dispatch and slot-array traffic
//! disappear. This is the levelized compiled-code simulation approach
//! (Verilator / ESSENT lineage) applied to the bit-sliced evaluator:
//! the generated function still processes 64 lanes per call, and is
//! fully generic — inputs and outputs are plain word arrays in the
//! compiled circuit's binding order, with no domain semantics.
//!
//! Generated signature (all pointers are `u64` words):
//!
//! ```text
//! fn tick(reg_current: *const u64, reg_next: *mut u64,
//!         inputs: *const u64 /* input_count */,
//!         outputs: *mut u64  /* output_count */,
//!         lane_state: *mut u64 /* lane_state_words */)
//! ```
//!
//! `lane_state` is a buffer the caller owns and keeps between ticks, for
//! regions whose state nothing outside them reads
//! ([`crate::compile::CompiledRegion::state_is_private`]). A lowering
//! that computes in lanes rather than bit planes keeps its state there
//! and never transposes it, which on a real filter is worth about a
//! fifth of the whole substitution. One word per register, which is
//! exactly what the bit-planed form takes, so the size is the engine's
//! to know and only the layout is the lowering's.
//!
//! The caller (see [`crate::eval::CircuitState::process_step_jit`])
//! runs `ticks_per_step` calls, double-buffering the register arrays
//! exactly like the interpreter, so the two backends are semantically
//! interchangeable and are tested against each other.
//!
//! Compilation happens off the real-time thread; the finished
//! [`JitProgram`] is immutable and callable from the real-time thread
//! without allocation or locking.
// Cranelift's own types are re-exported below rather than merely used,
// because the lowering API is stated in them: a crate writing a lowering
// would otherwise have to depend on Cranelift itself and could end up on
// a different version of it than this one.

pub use cranelift_codegen::ir::{
    AbiParam, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, Value, types,
};
use cranelift_codegen::settings::{self, Configurable};
pub use cranelift_frontend::FunctionBuilder;
use cranelift_frontend::FunctionBuilderContext;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};
use uuid::Uuid;

use crate::compile::CompiledCircuit;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JitError {
    #[error("cranelift is not supported on this host: {0}")]
    HostUnsupported(String),
    #[error(
        "the lowering registered for {name:?} left no live block to carry on in; \
         it may branch, but it has to come back"
    )]
    LoweringLeftNoBlock { name: String },
    #[error("no helper named {name:?} was declared by this lowering")]
    UnknownHelper { name: String },
    #[error("helper {name:?} takes {want} word(s), given {got}")]
    HelperArity {
        name: String,
        want: usize,
        got: usize,
    },
    #[error(
        "two lowerings declared a helper named {name:?} with different \
         functions or shapes"
    )]
    HelperNameClash { name: String },
    #[error(
        "the lowering registered for {name:?} returned different values for output \
         {output} and register {register}, which share a slot: whichever were \
         written second would silently win"
    )]
    LoweringAliasDisagrees {
        name: String,
        output: usize,
        register: usize,
    },
    #[error(
        "the lowering registered for {name:?} returned {got} next-state value(s) \
         for a region owning {want} register(s)"
    )]
    LoweringStateArity {
        name: String,
        want: usize,
        got: usize,
    },
    #[error("cranelift codegen failed: {0}")]
    Codegen(String),
    #[error(
        "the lowering registered for {name:?} returned {got} value(s) for a region \n         with {want} declared output(s)"
    )]
    LoweringArity {
        name: String,
        want: usize,
        got: usize,
    },
    #[error("internal error: instruction references an undriven slot")]
    UndrivenSlot,
    #[error(
        "this circuit contains {0} lowered delay bank(s), which the JIT does not \n         yet evaluate; the interpreter runs it correctly"
    )]
    UnsupportedDelayBanks(usize),
}

/// A native function a lowering may call, with a fixed shape.
///
/// **One ABI, deliberately.** A lowering that could name any address
/// with any signature would put an unchecked `unsafe` behind a safe
/// registry call, and there is no reason to: everything a lowering
/// needs to hand to native code is a run of bit-plane words, and
/// everything it needs back is another run.
///
/// The three pointers are the input words, where the output words go,
/// and the region's persistent lane state — the buffer that survives
/// between ticks for a region nothing outside reads
/// ([`crate::compile::CompiledRegion::state_is_private`]). A helper for
/// a region without private state is still handed a pointer, to a
/// zero-length scratch, and must not touch it.
///
/// The helper is called from generated code, possibly on a realtime thread. It
/// must not allocate, must not panic, and must read exactly the input
/// words it was declared to read and write exactly the output words it
/// was declared to write.
pub type JitHelper = unsafe extern "C" fn(*const u64, *mut u64, *mut u64);

/// A helper together with the name generated code calls it by and the
/// buffer sizes the engine will hand it.
#[derive(Clone)]
pub struct JitSymbol {
    /// Unique within one compilation. Two lowerings declaring the same
    /// name with different functions is refused rather than resolved.
    pub name: String,
    pub helper: JitHelper,
    /// How many `u64` words the helper reads.
    pub inputs: usize,
    /// How many it writes.
    pub outputs: usize,
}

impl std::fmt::Debug for JitSymbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JitSymbol")
            .field("name", &self.name)
            .field("inputs", &self.inputs)
            .field("outputs", &self.outputs)
            .finish()
    }
}

pub use crate::lane_state::LaneStateBinding;

/// What a lowering is registered against.
///
/// The digest is part of the key on purpose. A revision says the author
/// *meant* the gates to change; the digest notices that they changed
/// whether or not anyone said so. Without it, editing a netlist and
/// forgetting to bump its revision would leave a native lowering in
/// place that no longer computes the same thing — and it would be right
/// in the interpreter and wrong in the JIT, which is the worst place
/// for a disagreement to hide.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IntrinsicKey {
    pub name: String,
    pub revision: u32,
    pub digest: Uuid,
}

/// What a lowering is handed: the builder to emit into, the SSA values
/// already holding the region's declared inputs, and whatever native
/// helpers it declared.
///
/// Each input value is one `I64` bit plane — bit *l* is lane *l* — in
/// the order the module declared its input ports.
pub struct JitLoweringCx<'a, 'b> {
    pub builder: &'a mut FunctionBuilder<'b>,
    /// One value per declared input, in declared order.
    pub inputs: &'a [Value],
    /// One value per owned register, holding what it latched last
    /// tick, in the order `CompiledRegion::state` lists them.
    pub state: &'a [Value],
    /// Persistent lane-major state for this region, when it has any.
    ///
    /// `Some` only where the region's state is private — nothing
    /// outside it reads any of those registers — and then a lowering
    /// may keep whatever it likes in these words between ticks instead
    /// of reading `state` and returning `next_state`. On a filter that
    /// is worth about a fifth of the whole substitution, because a
    /// transpose costs by boundary width and this takes the state off
    /// the boundary.
    ///
    /// A lowering that uses this must **still** return `next_state`
    /// values, because the register array stays authoritative — the
    /// reference interpreter and a JIT without lowerings both work off
    /// it exactly as before — and the two must agree.
    pub lane_state: Option<LaneState>,
    scratch_in: cranelift_codegen::ir::StackSlot,
    scratch_out: cranelift_codegen::ir::StackSlot,
    /// The helpers this lowering declared, by name.
    helpers: &'a BTreeMap<String, (cranelift_codegen::ir::FuncRef, usize, usize)>,
    pointer_type: cranelift_codegen::ir::Type,
}

impl JitLoweringCx<'_, '_> {
    /// Spills `values` to a stack slot, calls the named helper, and
    /// loads its results back as SSA values.
    ///
    /// This is the whole of the foreign-call story, on purpose. A
    /// lowering that wants native arithmetic wants exactly this — hand
    /// over some bit planes, get some back — and leaving it to each
    /// lowering to build its own stack slots and calls would be the
    /// same code written many times with different bugs in it.
    ///
    /// The buffers are the sizes the symbol declared. Passing a
    /// different number of values is refused here rather than
    /// discovered as memory corruption later.
    pub fn call_helper(&mut self, name: &str, values: &[Value]) -> Result<Vec<Value>, JitError> {
        let (func, inputs, outputs) =
            *self
                .helpers
                .get(name)
                .ok_or_else(|| JitError::UnknownHelper {
                    name: name.to_string(),
                })?;
        if values.len() != inputs {
            return Err(JitError::HelperArity {
                name: name.to_string(),
                want: inputs,
                got: values.len(),
            });
        }

        // One scratch pair for every call in this function, sized to the
        // largest helper declared. Calls are straight-line and a helper
        // touches only the words it declared, so no two are ever live at
        // once -- and a slot per call site would grow the frame by the
        // sum of them for no gain.
        let word = 8_u32;
        for (index, value) in values.iter().enumerate() {
            self.builder
                .ins()
                .stack_store(*value, self.scratch_in, (index as u32 * word) as i32);
        }
        let in_ptr = self
            .builder
            .ins()
            .stack_addr(self.pointer_type, self.scratch_in, 0);
        let out_ptr = self
            .builder
            .ins()
            .stack_addr(self.pointer_type, self.scratch_out, 0);
        // The region's persistent lane state, or the output scratch's
        // address for a region that has none -- a helper without state
        // must not touch it, and a pointer it cannot use is safer to
        // hand over than a null one it might.
        let state_ptr = match self.lane_state {
            Some(lanes) => lanes.pointer(self.builder),
            None => out_ptr,
        };
        self.builder.ins().call(func, &[in_ptr, out_ptr, state_ptr]);

        Ok((0..outputs)
            .map(|index| {
                self.builder.ins().stack_load(
                    types::I64,
                    self.scratch_out,
                    (index as u32 * word) as i32,
                )
            })
            .collect())
    }
}

/// Where a region's persistent lane words live, and how to reach them.
#[derive(Clone, Copy)]
pub struct LaneState {
    base: Value,
    offset: usize,
    words: usize,
}

impl LaneState {
    /// How many `u64` words this region has, sized by
    /// `crate::lane_state::words_for`.
    pub fn words(&self) -> usize {
        self.words
    }

    /// A pointer to the first of them, for handing to a native helper.
    pub fn pointer(&self, builder: &mut FunctionBuilder<'_>) -> Value {
        let offset = i64::try_from(self.offset * 8).expect("a sane offset");
        builder.ins().iadd_imm(self.base, offset)
    }
}

/// What a lowering hands back.
///
/// Two lists, because a region with state has two jobs: say what its
/// outputs are *this* tick, and say what its registers should hold
/// *next*. The gates do both, and a substitute has to do both or the
/// filter it replaced stops being a filter.
#[derive(Debug, Clone, Default)]
pub struct Lowered {
    /// One value per declared output, in declared order.
    pub outputs: Vec<Value>,
    /// One value per owned register, in the order
    /// [`crate::compile::CompiledRegion::state`] lists them. Empty for
    /// a region that owns none.
    pub next_state: Vec<Value>,
}
impl Lowered {
    /// A region with no state: outputs only.
    pub fn outputs(outputs: Vec<Value>) -> Self {
        Self {
            outputs,
            next_state: Vec::new(),
        }
    }
}

/// A native replacement for one region's gates.
///
/// **The gates remain the definition.** Whatever this emits has to
/// compute the same 64 lanes they would, for every input, or the JIT
/// and the interpreter disagree and only one of them is checked. There
/// is no way for the engine to verify that; registering a lowering is
/// an assertion, and the crate that makes it owns it.
///
/// # What `emit` may do to the builder
///
/// The surrounding code is one straight-line block, and emission
/// continues in whatever block is current when `emit` returns. So:
///
/// - Emitting instructions into the current block is always fine, and
///   is what a lowering normally does.
/// - Control flow is allowed, but `emit` must leave a **live** block
///   current — one it has not terminated — and the values it returns
///   must be defined on every path reaching that block. A lowering that
///   branches away and never comes back leaves the rest of the tick
///   unreachable, and the function would either fail verification or
///   quietly compute nothing.
///
/// The second of those is checked, because the failure is otherwise a
/// panic somewhere far away in Cranelift with nothing pointing back
/// here.
pub trait JitLowering: Send + Sync {
    /// Native helpers this lowering will call, registered with the JIT
    /// before any code is generated. Empty for a lowering that emits
    /// only ordinary instructions.
    fn symbols(&self) -> Vec<JitSymbol> {
        Vec::new()
    }

    /// Emits the replacement and returns one value per declared output,
    /// in declared order.
    fn emit(&self, cx: JitLoweringCx<'_, '_>) -> Result<Lowered, JitError>;
}

impl<F> JitLowering for F
where
    F: Fn(JitLoweringCx<'_, '_>) -> Result<Lowered, JitError> + Send + Sync,
{
    fn emit(&self, cx: JitLoweringCx<'_, '_>) -> Result<Lowered, JitError> {
        self(cx)
    }
}

/// Native lowerings a JIT compilation may use, looked up by opaque key.
///
/// The engine holds keys and takes no view on what any of them means.
/// Which names exist, and what they compute, belongs entirely to
/// whatever crate builds the circuits.
#[derive(Default)]
pub struct JitRegistry {
    lowerings: BTreeMap<IntrinsicKey, Box<dyn JitLowering>>,
}

impl JitRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a lowering. Replaces any previous one for the same
    /// key.
    pub fn insert(&mut self, key: IntrinsicKey, lowering: Box<dyn JitLowering>) {
        self.lowerings.insert(key, lowering);
    }

    pub fn get(&self, key: &IntrinsicKey) -> Option<&dyn JitLowering> {
        self.lowerings.get(key).map(|boxed| boxed.as_ref())
    }

    /// Every registered lowering, so compilation can ask them all for
    /// the native helpers they will call before any code is generated.
    pub fn lowerings(&self) -> impl Iterator<Item = &dyn JitLowering> {
        self.lowerings.values().map(|boxed| boxed.as_ref())
    }

    pub fn is_empty(&self) -> bool {
        self.lowerings.is_empty()
    }
}

impl std::fmt::Debug for JitRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JitRegistry")
            .field("keys", &self.lowerings.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl PartialOrd for IntrinsicKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for IntrinsicKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.name, self.revision, self.digest.as_bytes()).cmp(&(
            &other.name,
            other.revision,
            other.digest.as_bytes(),
        ))
    }
}

type TickFn = unsafe extern "C" fn(*const u64, *mut u64, *const u64, *mut u64, *mut u64);

/// A natively compiled circuit tick function. Keeps the backing
/// executable memory (`JITModule`) alive for as long as the function
/// pointer may be called.
pub struct JitProgram {
    pub(crate) program_id: Uuid,
    pub(crate) ticks_per_step: u8,
    pub(crate) reg_count: usize,
    pub(crate) input_count: usize,
    pub(crate) output_count: usize,
    /// Regions a lowering was actually emitted for.
    substituted: Box<[u32]>,
    /// Where each substituted private-state region sits in the buffer.
    lane_state_layout: Box<[LaneStateBinding]>,
    /// Words of persistent lane state the caller has to keep.
    lane_state_words: usize,
    func: TickFn,
    /// Owns the executable memory `func` points into. Never used after
    /// construction, only dropped.
    _module: JITModule,
}

impl std::fmt::Debug for JitProgram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JitProgram")
            .field("program_id", &self.program_id)
            .field("ticks_per_step", &self.ticks_per_step)
            .field("reg_count", &self.reg_count)
            .finish_non_exhaustive()
    }
}

// SAFETY: after `finalize_definitions` the module's code memory is
// read+execute only and never mutated again; `func` is a plain
// function pointer into it. Calling it concurrently is safe (it only
// touches memory behind the caller-supplied pointers), and moving the
// program between threads moves only ownership of the mapping.
unsafe impl Send for JitProgram {}
unsafe impl Sync for JitProgram {}

impl JitProgram {
    /// Compiles the circuit's tick into native code, using no native
    /// lowerings: every region runs the gates it is made of.
    pub fn compile(circuit: &CompiledCircuit) -> Result<Self, JitError> {
        Self::compile_with(circuit, &JitRegistry::new())
    }

    /// Compiles the circuit's tick into native code, substituting a
    /// registered lowering for any region whose key it matches.
    ///
    /// A region with no match runs its gates, so an empty registry
    /// gives exactly what [`Self::compile`] gives.
    pub fn compile_with(
        circuit: &CompiledCircuit,
        registry: &JitRegistry,
    ) -> Result<Self, JitError> {
        // Refuse rather than silently omit. The generated code knows
        // only ordinary registers, so a lowered circuit would run with
        // its delay lines simply absent — wrong output, no error. JIT
        // compilation is best-effort with an interpreter fallback, so
        // saying no here costs speed and nothing else.
        if !circuit.banks.is_empty() {
            return Err(JitError::UnsupportedDelayBanks(circuit.banks.len()));
        }
        let mut flag_builder = settings::builder();
        flag_builder
            .set("opt_level", "speed")
            .map_err(|e| JitError::Codegen(e.to_string()))?;
        // Position-independent code is unnecessary for JIT pages.
        flag_builder
            .set("is_pic", "false")
            .map_err(|e| JitError::Codegen(e.to_string()))?;
        let isa_builder =
            cranelift_native::builder().map_err(|e| JitError::HostUnsupported(e.to_string()))?;
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| JitError::HostUnsupported(e.to_string()))?;

        // Helpers first: `JITBuilder::symbol` has to be called before
        // the module is built, so the registry is consulted before
        // anything else happens.
        let mut declared: BTreeMap<String, JitSymbol> = BTreeMap::new();
        for lowering in registry.lowerings() {
            for symbol in lowering.symbols() {
                match declared.get(&symbol.name) {
                    Some(existing)
                        if existing.helper as usize != symbol.helper as usize
                            || existing.inputs != symbol.inputs
                            || existing.outputs != symbol.outputs =>
                    {
                        return Err(JitError::HelperNameClash { name: symbol.name });
                    }
                    Some(_) => {}
                    None => {
                        declared.insert(symbol.name.clone(), symbol);
                    }
                }
            }
        }

        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        for symbol in declared.values() {
            builder.symbol(symbol.name.clone(), symbol.helper as *const u8);
        }
        let mut module = JITModule::new(builder);

        let pointer_type = module.target_config().pointer_type();
        let mut signature = module.make_signature();
        for _ in 0..5 {
            signature.params.push(AbiParam::new(pointer_type));
        }

        // Every helper is an import with the one shape helpers have.
        let mut helper_sig = module.make_signature();
        helper_sig.params.push(AbiParam::new(pointer_type));
        helper_sig.params.push(AbiParam::new(pointer_type));
        helper_sig.params.push(AbiParam::new(pointer_type));
        let mut helper_ids: Vec<(String, cranelift_module::FuncId, usize, usize)> = Vec::new();
        for symbol in declared.values() {
            let id = module
                .declare_function(&symbol.name, Linkage::Import, &helper_sig)
                .map_err(|e| JitError::Codegen(e.to_string()))?;
            helper_ids.push((symbol.name.clone(), id, symbol.inputs, symbol.outputs));
        }

        let func_id = module
            .declare_function("circuit_tick", Linkage::Local, &signature)
            .map_err(|e| JitError::Codegen(e.to_string()))?;

        let mut context = module.make_context();
        context.func.signature = signature;

        let substituted: Vec<u32>;
        let lane_state_map: BTreeMap<u32, (usize, usize)>;
        let mut builder_context = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut context.func, &mut builder_context);
            let block = b.create_block();
            b.append_block_params_for_function_params(block);
            b.switch_to_block(block);
            b.seal_block(block);

            let reg_current = b.block_params(block)[0];
            let reg_next = b.block_params(block)[1];
            let inputs = b.block_params(block)[2];
            let outputs = b.block_params(block)[3];
            let lane_state_base = b.block_params(block)[4];

            // Every driven slot becomes exactly one SSA value.
            // Register q slots are the first `regs.len()` slots.
            let mut value_of: Vec<Option<Value>> = vec![None; circuit.slot_count as usize];
            for (reg_index, slot) in value_of.iter_mut().take(circuit.regs.len()).enumerate() {
                let value = b.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    reg_current,
                    (reg_index * 8) as i32,
                );
                *slot = Some(value);
            }
            for (input_index, binding) in circuit.inputs.iter().enumerate() {
                let value = b.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    inputs,
                    (input_index * 8) as i32,
                );
                value_of[binding.slot as usize] = Some(value);
            }

            let mut helpers: BTreeMap<String, (cranelift_codegen::ir::FuncRef, usize, usize)> =
                BTreeMap::new();
            for (name, id, inputs, outputs) in &helper_ids {
                let reference = module.declare_func_in_func(*id, b.func);
                helpers.insert(name.clone(), (reference, *inputs, *outputs));
            }
            // Sized to the largest helper any registered lowering
            // declared, and created once: helper calls are straight-line
            // and each touches only the words it declared, so no two
            // buffers are ever live at the same time.
            let widest_in = declared.values().map(|s| s.inputs).max().unwrap_or(0);
            let widest_out = declared.values().map(|s| s.outputs).max().unwrap_or(0);
            let scratch = |b: &mut FunctionBuilder<'_>, words: usize| {
                b.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    (words.max(1) * 8) as u32,
                    3,
                ))
            };
            let scratch_in = scratch(&mut b, widest_in);
            let scratch_out = scratch(&mut b, widest_out);

            let mut emitter = Emitter {
                circuit,
                registry,
                helpers,
                pointer_type,
                scratch_in,
                scratch_out,
                lane_state_base,
                lane_state: BTreeMap::new(),
                substituted: Vec::new(),
            };
            emitter.emit(&mut b, &mut value_of, &circuit.instrs)?;
            lane_state_map = emitter.lane_state.clone();
            substituted = emitter.substituted;

            for (reg_index, spec) in circuit.regs.iter().enumerate() {
                let value = value_of[spec.d_slot as usize].ok_or(JitError::UndrivenSlot)?;
                b.ins().store(
                    MemFlagsData::trusted(),
                    value,
                    reg_next,
                    (reg_index * 8) as i32,
                );
            }

            for (output_index, binding) in circuit.outputs.iter().enumerate() {
                let value = value_of[binding.slot as usize].ok_or(JitError::UndrivenSlot)?;
                b.ins().store(
                    MemFlagsData::trusted(),
                    value,
                    outputs,
                    (output_index * 8) as i32,
                );
            }

            b.ins().return_(&[]);
            b.finalize();
        }

        module
            .define_function(func_id, &mut context)
            .map_err(|e| JitError::Codegen(e.to_string()))?;
        module.clear_context(&mut context);
        module
            .finalize_definitions()
            .map_err(|e| JitError::Codegen(e.to_string()))?;

        let code = module.get_finalized_function(func_id);
        // SAFETY: the function was defined with exactly the TickFn
        // signature above.
        let func: TickFn = unsafe { std::mem::transmute(code) };

        Ok(Self {
            program_id: circuit.program_id,
            ticks_per_step: circuit.ticks_per_step,
            reg_count: circuit.regs.len(),
            input_count: circuit.inputs.len(),
            output_count: circuit.outputs.len(),
            substituted: substituted.into_boxed_slice(),
            // Each region's slice, and what the bytes in it mean. The
            // engine sizes the slice -- 64 lanes of `ceil(n/8)` bytes --
            // and carries the region's key so a state cannot be handed
            // to a program that carves the same and means otherwise.
            lane_state_layout: lane_state_map
                .iter()
                .map(|(region, (offset, words))| {
                    let compiled = &circuit.regions[*region as usize];
                    LaneStateBinding {
                        offset: *offset,
                        words: *words,
                        name: compiled.name.clone(),
                        revision: compiled.revision,
                        digest: compiled.digest,
                    }
                })
                .collect(),
            lane_state_words: lane_state_map.values().map(|(_, words)| *words).sum(),
            func,
            _module: module,
        })
    }

    /// Which of the circuit's regions this program actually substituted
    /// native code for, as indices into
    /// [`crate::compile::CompiledCircuit::regions`].
    ///
    /// **This, and not the registry, is what a cost estimate may
    /// discount.** A registry says what lowerings exist; a key that does
    /// not match is a lowering that exists and was not used, and the
    /// gates ran instead. Anything charging less for a substitution has
    /// to ask what happened rather than what was on offer, or it will
    /// eventually admit a circuit on the strength of one that never
    /// took place.
    pub fn substituted_regions(&self) -> &[u32] {
        &self.substituted
    }

    pub fn ticks_per_step(&self) -> u8 {
        self.ticks_per_step
    }

    /// Where each substituted region's private state sits in the buffer,
    /// with its word offset, word count and identity.
    ///
    /// **The layout inside a region's words is contracted, not free.**
    /// A region with `n` registers takes [`crate::lane_state::words_for`]
    /// `u64` words. Each lane owns `ceil(n / 8)` whole bytes, as computed
    /// by [`crate::lane_state::bytes_per_lane`]; lane `l` starts at byte
    /// `l * ceil(n / 8)` within the region. This byte-granular layout lets
    /// the engine reset one lane without knowing what its bytes mean.
    pub fn lane_state_regions(&self) -> &[LaneStateBinding] {
        &self.lane_state_layout
    }

    /// Whether a substituted region keeps private state outside the
    /// register array.
    pub fn keeps_private_state(&self) -> bool {
        self.lane_state_words > 0
    }

    /// Number of persistent `u64` lane-state words required, or zero if
    /// no substituted region keeps private state. The caller preserves
    /// this buffer between ticks.
    pub fn lane_state_words(&self) -> usize {
        self.lane_state_words
    }

    /// Runs one tick.
    ///
    /// # Safety
    /// `reg_current`/`reg_next` must hold at least `reg_count` words,
    /// `inputs` at least `input_count` words, `outputs` at least
    /// `output_count` words and `lane_state` at least
    /// `lane_state_words`; the regions must not overlap. `lane_state`
    /// must be the same buffer as the previous tick's, or the state it
    /// holds is somebody else's.
    pub(crate) unsafe fn tick(
        &self,
        reg_current: *const u64,
        reg_next: *mut u64,
        inputs: *const u64,
        outputs: *mut u64,
        lane_state: *mut u64,
    ) {
        unsafe { (self.func)(reg_current, reg_next, inputs, outputs, lane_state) }
    }
}

/// Emits one straight-line block of instructions.
///
/// A region is where the two paths part. If the registry has a lowering
/// under the region's key, its values are taken as the region's outputs
/// and the region's own gates are never emitted at all. If it has none,
/// the gates are emitted like any others — which is why an empty
/// registry produces exactly the code this backend produced before
/// regions existed.
///
/// The recursion is one level deep: a fallback never contains a region,
/// because nesting is claimed by the outermost at flattening time.
/// Everything emission needs beyond the builder, gathered so the
/// recursion carries one reference rather than seven.
struct Emitter<'a> {
    circuit: &'a CompiledCircuit,
    registry: &'a JitRegistry,
    helpers: BTreeMap<String, (cranelift_codegen::ir::FuncRef, usize, usize)>,
    pointer_type: cranelift_codegen::ir::Type,
    /// One scratch pair for every helper call in this function, sized
    /// to the largest helper declared.
    scratch_in: cranelift_codegen::ir::StackSlot,
    scratch_out: cranelift_codegen::ir::StackSlot,
    /// The pointer the tick was handed for persistent lane state.
    lane_state_base: Value,
    /// Where each substituted private-state region keeps its lane
    /// state: `region -> (word offset, word count)`. Filled as regions
    /// are emitted, so the offsets follow emission order.
    lane_state: BTreeMap<u32, (usize, usize)>,
    /// Regions a lowering was actually emitted for.
    ///
    /// **The only honest source of a discount.** A registry says what
    /// lowerings *exist*; this says which ones were used, and those are
    /// different whenever a key does not match. Admission that trusted
    /// the registry would let a circuit through on the strength of a
    /// substitution that never happened.
    substituted: Vec<u32>,
}

impl Emitter<'_> {
    fn emit(
        &mut self,
        b: &mut FunctionBuilder<'_>,
        value_of: &mut [Option<Value>],
        instrs: &[crate::compile::Instr],
    ) -> Result<(), JitError> {
        for instr in instrs {
            match *instr {
                crate::compile::Instr::Nand { a, b: bi, out } => {
                    let av = value_of[a as usize].ok_or(JitError::UndrivenSlot)?;
                    let bv = value_of[bi as usize].ok_or(JitError::UndrivenSlot)?;
                    let and = b.ins().band(av, bv);
                    let nand = b.ins().bnot(and);
                    value_of[out as usize] = Some(nand);
                }
                crate::compile::Instr::FullAdder {
                    a,
                    b: bi,
                    cin,
                    sum,
                    cout,
                } => {
                    let av = value_of[a as usize].ok_or(JitError::UndrivenSlot)?;
                    let bv = value_of[bi as usize].ok_or(JitError::UndrivenSlot)?;
                    let cv = value_of[cin as usize].ok_or(JitError::UndrivenSlot)?;
                    let p = b.ins().bxor(av, bv);
                    let s = b.ins().bxor(p, cv);
                    let g = b.ins().band(av, bv);
                    let t = b.ins().band(p, cv);
                    let carry = b.ins().bor(g, t);
                    value_of[sum as usize] = Some(s);
                    value_of[cout as usize] = Some(carry);
                }
                crate::compile::Instr::Xor { a, b: bi, out } => {
                    let av = value_of[a as usize].ok_or(JitError::UndrivenSlot)?;
                    let bv = value_of[bi as usize].ok_or(JitError::UndrivenSlot)?;
                    value_of[out as usize] = Some(b.ins().bxor(av, bv));
                }
                crate::compile::Instr::And { a, b: bi, out } => {
                    let av = value_of[a as usize].ok_or(JitError::UndrivenSlot)?;
                    let bv = value_of[bi as usize].ok_or(JitError::UndrivenSlot)?;
                    value_of[out as usize] = Some(b.ins().band(av, bv));
                }
                crate::compile::Instr::Not { a, out } => {
                    let av = value_of[a as usize].ok_or(JitError::UndrivenSlot)?;
                    value_of[out as usize] = Some(b.ins().bnot(av));
                }
                crate::compile::Instr::Region {
                    region: region_index,
                } => {
                    let region = region_index;
                    let region = &self.circuit.regions[region as usize];
                    let key = IntrinsicKey {
                        name: region.name.clone(),
                        revision: region.revision,
                        digest: region.digest,
                    };
                    match self.registry.get(&key) {
                        Some(lowering) => {
                            let inputs = region
                                .inputs
                                .iter()
                                .map(|slot| value_of[*slot as usize].ok_or(JitError::UndrivenSlot))
                                .collect::<Result<Vec<_>, _>>()?;
                            // What each owned register latched last
                            // tick. Loaded before anything runs, so it
                            // is there whether the gates ran or not.
                            let state = region
                                .state
                                .iter()
                                .map(|binding| {
                                    value_of[binding.q_slot as usize].ok_or(JitError::UndrivenSlot)
                                })
                                .collect::<Result<Vec<_>, _>>()?;
                            // A region whose state nothing outside it
                            // reads gets a slice of the persistent
                            // buffer, allocated the first time it is
                            // emitted. Byte-granular storage for each
                            // lane lets the engine size it without
                            // knowing what the lowering will put there.
                            let lane_state = if region.state_is_private {
                                let next = self
                                    .lane_state
                                    .values()
                                    .map(|(offset, words)| offset + words)
                                    .max()
                                    .unwrap_or(0);
                                let (offset, words) =
                                    *self.lane_state.entry(region_index).or_insert((
                                        next,
                                        crate::lane_state::words_for(region.state.len()),
                                    ));
                                Some(LaneState {
                                    base: self.lane_state_base,
                                    offset,
                                    words,
                                })
                            } else {
                                None
                            };
                            let produced = lowering.emit(JitLoweringCx {
                                builder: b,
                                inputs: &inputs,
                                state: &state,
                                lane_state,
                                scratch_in: self.scratch_in,
                                scratch_out: self.scratch_out,
                                helpers: &self.helpers,
                                pointer_type: self.pointer_type,
                            })?;
                            // The rest of the tick is emitted into whatever
                            // block is current now. If the lowering left it
                            // terminated, the next instruction added would
                            // panic inside the code generator with nothing
                            // pointing back at the lowering that did it.
                            let terminated = b.current_block().is_some_and(|block| {
                                b.func.layout.last_inst(block).is_some_and(|inst| {
                                    b.func.dfg.insts[inst].opcode().is_terminator()
                                })
                            });
                            if terminated {
                                return Err(JitError::LoweringLeftNoBlock {
                                    name: region.name.clone(),
                                });
                            }
                            if produced.outputs.len() != region.outputs.len() {
                                return Err(JitError::LoweringArity {
                                    name: region.name.clone(),
                                    want: region.outputs.len(),
                                    got: produced.outputs.len(),
                                });
                            }
                            if produced.next_state.len() != region.state.len() {
                                return Err(JitError::LoweringStateArity {
                                    name: region.name.clone(),
                                    want: region.state.len(),
                                    got: produced.next_state.len(),
                                });
                            }
                            self.substituted.push(region_index);
                            // A region whose output net is also a
                            // register's `d` is the ordinary shape of
                            // "the output is the new state", not an
                            // exotic one. Both writes land on the same
                            // slot, so whichever runs second wins, and
                            // a lowering returning two different values
                            // there would be silently half ignored.
                            //
                            // Requiring the same SSA value is stricter
                            // than requiring the same arithmetic, and
                            // deliberately: a lowering that means them
                            // to be equal computes them once, and one
                            // that computes them twice can say so by
                            // returning the value it already has.
                            for (index, slot) in region.outputs.iter().enumerate() {
                                for (position, binding) in region.state.iter().enumerate() {
                                    if binding.d_slot == *slot
                                        && produced.outputs[index] != produced.next_state[position]
                                    {
                                        return Err(JitError::LoweringAliasDisagrees {
                                            name: region.name.clone(),
                                            output: index,
                                            register: position,
                                        });
                                    }
                                }
                            }
                            for (slot, value) in region.outputs.iter().zip(produced.outputs) {
                                value_of[*slot as usize] = Some(value);
                            }
                            // Next state goes exactly where the gates
                            // would have put it, so the ordinary latch
                            // at the end of the tick picks it up and
                            // nothing else has to know a substitution
                            // happened.
                            for (binding, value) in region.state.iter().zip(produced.next_state) {
                                value_of[binding.d_slot as usize] = Some(value);
                            }
                        }
                        None => {
                            let fallback = region.fallback.clone();
                            self.emit(b, value_of, &fallback)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
