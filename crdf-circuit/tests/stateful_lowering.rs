//! **A region that remembers something, replaced.**
//!
//! A stateless substitution only has to get this tick right. One with
//! state has to get next tick right too, and getting that wrong is the
//! quietest kind of wrong: the first sample matches, and the circuit
//! drifts.
//!
//! The region here is a one-bit counter with an enable — small enough to
//! read, and it fails visibly if next state is off by a tick.

#![cfg(feature = "jit")]

use crdf_circuit::compile::{CompiledCircuit, flatten};
use crdf_circuit::eval::CircuitState;
use crdf_circuit::jit::{
    InstBuilder, IntrinsicKey, JitLoweringCx, JitProgram, JitRegistry, Lowered,
};
use crdf_circuit::model::{
    CircuitAsset, Endpoint, IntrinsicDecl, Module, ModuleBuilder, ModuleId, ModuleLibrary,
    PortDirection,
};

/// `q' = q XOR enable`, with `out = q'`.
///
/// Built from NANDs so it is a real circuit rather than a special case:
/// the XOR is the canonical four-NAND one, and its result drives both
/// the output and the register.
fn toggle(declared: bool) -> Module {
    let mut m = ModuleBuilder::new("toggle");
    let enable = m.input("enable");
    let out = m.output("out");
    let reg = m.reg(false);

    // n1 = nand(q, e); n2 = nand(q, n1); n3 = nand(e, n1);
    // xor = nand(n2, n3)
    let n1 = m.nand();
    m.wire(Endpoint::reg_q(reg), Endpoint::nand_a(n1));
    m.wire(Endpoint::module(enable), Endpoint::nand_b(n1));
    let n2 = m.nand();
    m.wire(Endpoint::reg_q(reg), Endpoint::nand_a(n2));
    m.wire(Endpoint::nand_y(n1), Endpoint::nand_b(n2));
    let n3 = m.nand();
    m.wire(Endpoint::module(enable), Endpoint::nand_a(n3));
    m.wire(Endpoint::nand_y(n1), Endpoint::nand_b(n3));
    let xor = m.nand();
    m.wire(Endpoint::nand_y(n2), Endpoint::nand_a(xor));
    m.wire(Endpoint::nand_y(n3), Endpoint::nand_b(xor));

    m.wire(Endpoint::nand_y(xor), Endpoint::reg_d(reg));
    m.wire(Endpoint::nand_y(xor), Endpoint::module(out));

    let mut module = m.finish();
    if declared {
        module.intrinsic = Some(IntrinsicDecl {
            name: "test.toggle".into(),
            revision: 1,
            inputs: vec![enable],
            outputs: vec![out],
        });
    }
    module
}

/// `out = q ^ enable`, with the register kept lane-major in the private
/// buffer.
///
/// A native helper rather than emitted instructions, because that is
/// what the layout is for: lane `l` owns byte `l`, and reaching sixty-
/// four separate bytes from generated code would be sixty-four stores.
/// Written the way a real lowering is written, so the test exercises
/// the contract instead of a shape that only looks like it.
///
/// # Safety
/// `input` must point to one readable `u64`, `output` to one writable
/// one, and `state` to a buffer of `lane_state::words_for(1)` words.
unsafe extern "C" fn toggle_helper(input: *const u64, output: *mut u64, state: *mut u64) {
    // SAFETY: the caller guarantees one readable word.
    let enable = unsafe { *input };
    let mut result = 0_u64;
    for lane in 0..64 {
        // SAFETY: the caller guarantees the buffer.
        let slot = unsafe { crdf_circuit::lane_state::lane_mut(state, 1, lane) };
        // SAFETY: `slot` is this lane's byte.
        let held = unsafe { *slot } & 1;
        let bit = ((enable >> lane) & 1) as u8;
        let next = held ^ bit;
        // SAFETY: as above.
        unsafe { *slot = next };
        result |= u64::from(next) << lane;
    }
    // SAFETY: the caller guarantees one writable word.
    unsafe { *output = result };
}

/// A lowering that keeps its state in the private buffer, through the
/// helper above.
struct ToggleLowering;

impl crdf_circuit::jit::JitLowering for ToggleLowering {
    fn symbols(&self) -> Vec<crdf_circuit::jit::JitSymbol> {
        vec![crdf_circuit::jit::JitSymbol {
            name: "test.toggle_helper".into(),
            helper: toggle_helper as crdf_circuit::jit::JitHelper,
            inputs: 1,
            outputs: 1,
        }]
    }

    fn emit(&self, mut cx: JitLoweringCx<'_, '_>) -> Result<Lowered, crdf_circuit::jit::JitError> {
        assert!(
            cx.lane_state.is_some(),
            "this region's state is private, so there is a buffer"
        );
        let inputs = cx.inputs.to_vec();
        let outputs = cx.call_helper("test.toggle_helper", &inputs)?;
        // The toggle's output *is* its next state -- one gate drives
        // both the output port and the register's `d`, so they are the
        // same slot. A lowering that returned something else for the
        // register would be half ignored, and the engine refuses it
        // rather than letting that happen quietly.
        //
        // So the register array ends up holding the true state as well
        // as the buffer. That is not a problem: private state means
        // nothing outside *reads* the registers, not that they must be
        // stale.
        let next_state = outputs.clone();
        Ok(Lowered {
            outputs,
            next_state,
        })
    }
}

/// One instance, so the top has something to expose.
fn top(inner: ModuleId, lib: &ModuleLibrary) -> Module {
    let mut m = ModuleBuilder::new("top");
    let enable = m.input("enable");
    let out = m.output("out");
    let ports = &lib.get(inner).expect("inner").ports;
    let find = |dir: PortDirection| {
        ports
            .iter()
            .find(|p| p.direction == dir)
            .expect("a port")
            .id
    };
    let cell = m.instance(inner);
    m.wire(
        Endpoint::module(enable),
        Endpoint::inner(cell, find(PortDirection::Input)),
    );
    m.wire(
        Endpoint::inner(cell, find(PortDirection::Output)),
        Endpoint::module(out),
    );
    m.finish()
}

fn asset(declared: bool) -> CircuitAsset {
    let mut lib = ModuleLibrary::new();
    let inner = toggle(declared);
    let inner_id = inner.id;
    lib.insert(inner);
    let module = top(inner_id, &lib);
    let top_id = module.id;
    lib.insert(module);
    CircuitAsset::new(top_id, 1, lib)
}

/// `out = q ^ enable`, and the register takes the same value.
fn toggle_natively(cx: JitLoweringCx<'_, '_>) -> Result<Lowered, crdf_circuit::jit::JitError> {
    let next = cx.builder.ins().bxor(cx.state[0], cx.inputs[0]);
    Ok(Lowered {
        outputs: vec![next],
        next_state: vec![next],
    })
}

/// Runs `steps` ticks with `enable` held, collecting `out` each tick.
fn run(
    circuit: &CompiledCircuit,
    program: Option<&JitProgram>,
    enable: u64,
    steps: usize,
) -> Vec<u64> {
    let mut state = CircuitState::new(circuit);
    let index = circuit.input_index("enable").expect("an input");
    for lane in 0..64 {
        state.set_level(circuit, index, lane, (enable >> lane) & 1 == 1);
    }
    let out = circuit.output_index("out").expect("an output");
    (0..steps)
        .map(|_| {
            match program {
                Some(program) => state.process_step_jit(program),
                None => state.process_step(circuit),
            }
            state.output_word(out)
        })
        .collect()
}

/// **A region owning a register is offered, and says which.**
#[test]
fn a_region_that_owns_a_register_reports_it() {
    let asset = asset(true);
    let flat = flatten(asset.root, &asset.library).expect("flattens");
    assert_eq!(flat.regs().len(), 1);
    assert_ne!(
        flat.regs()[0].d,
        flat.regs()[0].q,
        "real state, not a constant"
    );

    let circuit = CompiledCircuit::compile(&asset).expect("compiles");
    assert_eq!(circuit.regions().len(), 1, "offered despite the state");
    let region = &circuit.regions()[0];
    assert_eq!(region.state.len(), 1, "one register owned");
    assert_ne!(
        region.state[0].q_slot, region.state[0].d_slot,
        "the two ends of a register are different slots"
    );
    assert!(!region.state[0].init);
}

/// **The lowering agrees with the gates over time, not just at the
/// first sample.**
///
/// A wrong next-state is invisible on tick one and obvious by tick two,
/// so this compares a run rather than a value.
#[test]
fn a_stateful_lowering_agrees_over_a_run() {
    let asset = asset(true);
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");

    let mut registry = JitRegistry::new();
    let region = &circuit.regions()[0];
    registry.insert(
        IntrinsicKey {
            name: region.name.clone(),
            revision: region.revision,
            digest: region.digest,
        },
        Box::new(toggle_natively),
    );

    let plain = JitProgram::compile(&circuit).expect("compiles");
    let lowered = JitProgram::compile_with(&circuit, &registry).expect("compiles");
    assert_eq!(
        lowered.substituted_regions(),
        &[0],
        "the lowering has to have been used, or this compares the gates \
         with themselves"
    );

    for enable in [0, u64::MAX, 0x5555_5555_5555_5555, 0xdead_beef_1234_5678] {
        let reference = run(&circuit, None, enable, 8);
        assert_eq!(
            run(&circuit, Some(&plain), enable, 8),
            reference,
            "jit, gates"
        );
        assert_eq!(
            run(&circuit, Some(&lowered), enable, 8),
            reference,
            "the lowering drifts from the gates it replaces"
        );
    }

    // And the run is actually doing something: with enable set the
    // output has to alternate, or all three agree on a constant and the
    // comparison proves nothing.
    let reference = run(&circuit, None, u64::MAX, 4);
    assert_eq!(reference, vec![u64::MAX, 0, u64::MAX, 0]);
}

/// **A lowering that forgets its state is refused, not run.**
#[test]
fn a_lowering_that_returns_no_next_state_is_refused() {
    let asset = asset(true);
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");
    let region = &circuit.regions()[0];

    let mut registry = JitRegistry::new();
    registry.insert(
        IntrinsicKey {
            name: region.name.clone(),
            revision: region.revision,
            digest: region.digest,
        },
        Box::new(
            |cx: JitLoweringCx<'_, '_>| -> Result<Lowered, crdf_circuit::jit::JitError> {
                let next = cx.builder.ins().bxor(cx.state[0], cx.inputs[0]);
                Ok(Lowered::outputs(vec![next]))
            },
        ),
    );

    assert!(matches!(
        JitProgram::compile_with(&circuit, &registry),
        Err(crdf_circuit::jit::JitError::LoweringStateArity { .. })
    ));
}

/// **When an output and a register share a slot, both have to agree.**
///
/// This region is the ordinary shape of "the output is the new state":
/// one gate drives the output port and the register's `d`, so they
/// resolve to the same net and the same slot. Two writes land there,
/// and whichever ran second would win — so a lowering returning
/// different values for them would be half ignored, silently.
///
/// Refused instead. The rule is the same SSA value, not the same
/// arithmetic, which is stricter on purpose: a lowering that means them
/// to be equal computes them once.
#[test]
fn an_output_sharing_a_slot_with_a_register_must_agree() {
    let asset = asset(true);
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");
    let region = &circuit.regions()[0];
    assert_eq!(
        region.outputs[0], region.state[0].d_slot,
        "this test needs the alias to exist, and in a toggle it does"
    );

    let mut registry = JitRegistry::new();
    registry.insert(
        IntrinsicKey {
            name: region.name.clone(),
            revision: region.revision,
            digest: region.digest,
        },
        Box::new(
            |cx: JitLoweringCx<'_, '_>| -> Result<Lowered, crdf_circuit::jit::JitError> {
                // Right arithmetic, computed twice: two SSA values that
                // happen to agree.
                let one = cx.builder.ins().bxor(cx.state[0], cx.inputs[0]);
                let other = cx.builder.ins().bxor(cx.state[0], cx.inputs[0]);
                Ok(Lowered {
                    outputs: vec![one],
                    next_state: vec![other],
                })
            },
        ),
    );

    assert!(matches!(
        JitProgram::compile_with(&circuit, &registry),
        Err(crdf_circuit::jit::JitError::LoweringAliasDisagrees { .. })
    ));
}

/// **A region whose state nobody else reads may hold it how it likes.**
///
/// The toggle's register is read only by the gates that drive it — its
/// output is the XOR, not the `q` — so nothing outside can tell what
/// representation the state is in. That is the condition for a backend
/// to keep it lane-major between ticks instead of transposing it in and
/// out of the register array every tick, which on a real filter is
/// worth about a fifth of the total.
#[test]
fn a_region_whose_state_leaves_it_is_not_private() {
    let asset = asset(true);
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");
    assert!(
        circuit.regions()[0].state_is_private,
        "the toggle's q is read only by its own gates"
    );

    // The same circuit with the register's `q` exposed at the top: now
    // something outside reads it, and the representation stops being
    // the region's own business.
    let mut lib = ModuleLibrary::new();
    let mut m = ModuleBuilder::new("leaky");
    let enable = m.input("enable");
    let out = m.output("out");
    let held = m.output("held");
    let reg = m.reg(false);
    let n1 = m.nand();
    m.wire(Endpoint::reg_q(reg), Endpoint::nand_a(n1));
    m.wire(Endpoint::module(enable), Endpoint::nand_b(n1));
    let n2 = m.nand();
    m.wire(Endpoint::reg_q(reg), Endpoint::nand_a(n2));
    m.wire(Endpoint::nand_y(n1), Endpoint::nand_b(n2));
    let n3 = m.nand();
    m.wire(Endpoint::module(enable), Endpoint::nand_a(n3));
    m.wire(Endpoint::nand_y(n1), Endpoint::nand_b(n3));
    let xor = m.nand();
    m.wire(Endpoint::nand_y(n2), Endpoint::nand_a(xor));
    m.wire(Endpoint::nand_y(n3), Endpoint::nand_b(xor));
    m.wire(Endpoint::nand_y(xor), Endpoint::reg_d(reg));
    m.wire(Endpoint::nand_y(xor), Endpoint::module(out));
    // The one difference: the register's own value goes out as well.
    m.wire(Endpoint::reg_q(reg), Endpoint::module(held));
    let mut inner = m.finish();
    inner.intrinsic = Some(IntrinsicDecl {
        name: "test.leaky".into(),
        revision: 1,
        inputs: vec![enable],
        outputs: vec![out, held],
    });
    let inner_id = inner.id;
    lib.insert(inner);

    let mut t = ModuleBuilder::new("top");
    let x = t.input("enable");
    let y = t.output("out");
    let z = t.output("held");
    let ports = lib.get(inner_id).expect("inner").ports.clone();
    let find = |name: &str| ports.iter().find(|p| p.name == name).expect("a port").id;
    let cell = t.instance(inner_id);
    t.wire(Endpoint::module(x), Endpoint::inner(cell, find("enable")));
    t.wire(Endpoint::inner(cell, find("out")), Endpoint::module(y));
    t.wire(Endpoint::inner(cell, find("held")), Endpoint::module(z));
    let top_id = t.id();
    lib.insert(t.finish());

    let leaky = CompiledCircuit::compile(&CircuitAsset::new(top_id, 1, lib)).expect("compiles");
    // An output that is a register's q is refused as a region anyway,
    // by the rule that every declared output must be gate-produced --
    // so the privacy question never arises for it, which is itself the
    // answer this test wanted.
    assert!(
        leaky.regions().is_empty() || !leaky.regions()[0].state_is_private,
        "a register whose value leaves the region is not private state"
    );
}

/// **A lowering can keep its state where it likes, and still agree.**
///
/// The toggle's register is private, so the lowering is handed a slice
/// of a buffer that survives between ticks. Here it keeps the same bit
/// there that the register holds — a lane-major filter would keep
/// something quite different — and the point is that it works: the
/// register array is still latched from `next_state`, so the reference
/// interpreter sees exactly what it always did, and a run of eight
/// ticks agrees.
///
/// That last part is what makes the buffer safe to add. It is an
/// optimisation a backend may take, not a change to what a circuit
/// means.
#[test]
fn a_lowering_may_keep_state_in_its_own_buffer() {
    let asset = asset(true);
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");
    let region = &circuit.regions()[0];
    assert!(region.state_is_private);

    let mut registry = JitRegistry::new();
    registry.insert(
        IntrinsicKey {
            name: region.name.clone(),
            revision: region.revision,
            digest: region.digest,
        },
        Box::new(ToggleLowering),
    );

    let lowered = JitProgram::compile_with(&circuit, &registry).expect("compiles");
    assert_eq!(lowered.substituted_regions(), &[0]);
    assert_eq!(
        lowered.lane_state_words(),
        crdf_circuit::lane_state::words_for(1),
        "one register is a byte per lane, so eight words"
    );

    for enable in [0, u64::MAX, 0x5555_5555_5555_5555] {
        assert_eq!(
            run(&circuit, Some(&lowered), enable, 8),
            run(&circuit, None, enable, 8),
            "a lowering keeping its own state still has to agree with \
             the gates over a run"
        );
    }
}

/// **A state bound to one program's ABI refuses another's.**
///
/// Two programs from the same circuit -- one with lowerings, one
/// without -- share a `program_id` and would otherwise be swapped
/// freely. They are not interchangeable once one of them keeps a
/// region's state privately: the register array then holds that
/// region's init forever, so switching restarts that region and
/// switching back resumes the old state, with nothing raised either
/// way. Plausible output, and wrong.
#[test]
#[should_panic(expected = "not interchangeable")]
fn swapping_between_a_lowered_and_a_plain_program_is_refused() {
    let asset = asset(true);
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");
    let region = &circuit.regions()[0];

    let mut registry = JitRegistry::new();
    registry.insert(
        IntrinsicKey {
            name: region.name.clone(),
            revision: region.revision,
            digest: region.digest,
        },
        Box::new(ToggleLowering),
    );
    let lowered = JitProgram::compile_with(&circuit, &registry).expect("compiles");
    let plain = JitProgram::compile(&circuit).expect("compiles");

    let mut state = CircuitState::new(&circuit);
    state.process_step_jit(&lowered);
    state.process_step_jit(&plain);
}

/// **A full reset makes them interchangeable again.**
///
/// Once every lane is back to `init`, both representations hold the
/// same nothing and the register array is the whole truth once more.
#[test]
fn a_full_reset_lets_the_interpreter_take_over() {
    let asset = asset(true);
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");
    let region = &circuit.regions()[0];

    let mut registry = JitRegistry::new();
    registry.insert(
        IntrinsicKey {
            name: region.name.clone(),
            revision: region.revision,
            digest: region.digest,
        },
        Box::new(ToggleLowering),
    );
    let lowered = JitProgram::compile_with(&circuit, &registry).expect("compiles");

    let mut state = CircuitState::new(&circuit);
    let index = circuit.input_index("enable").expect("an input");
    for lane in 0..64 {
        state.set_level(&circuit, index, lane, true);
    }
    state.process_step_jit(&lowered);
    state.reset_all_lanes(&circuit);
    // No panic: the register array is the whole truth again.
    state.process_step(&circuit);
    let out = circuit.output_index("out").expect("an output");
    assert_eq!(
        state.output_word(out),
        u64::MAX,
        "a reset toggle with enable set gives one on every lane"
    );
}

/// **A full reset lifts the binding as well as the state.**
///
/// Once both representations are back to `init` there is nothing left
/// for a differently laid-out program to land on top of, so refusing
/// one would be refusing nothing. The earlier version cleared the state
/// and left the binding, and the test that was supposed to cover it
/// only ever switched to the interpreter.
#[test]
fn a_full_reset_lets_another_program_take_over() {
    let asset = asset(true);
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");
    let region = &circuit.regions()[0];

    let mut registry = JitRegistry::new();
    registry.insert(
        IntrinsicKey {
            name: region.name.clone(),
            revision: region.revision,
            digest: region.digest,
        },
        Box::new(ToggleLowering),
    );
    let lowered = JitProgram::compile_with(&circuit, &registry).expect("compiles");
    let plain = JitProgram::compile(&circuit).expect("compiles");

    let mut state = CircuitState::new(&circuit);
    state.process_step_jit(&lowered);
    state.reset_all_lanes(&circuit);
    // No panic: nothing of the old encoding survives to be misread.
    state.process_step_jit(&plain);
}

/// **The binding is the region's identity, not only its size.**
///
/// Two programs can carve the buffer identically and mean entirely
/// different things by the bytes — different regions of the same size,
/// or the same region lowered differently. A state carried between them
/// would be reading somebody else's encoding with nothing to say so, so
/// the binding carries the key as well as the place.
#[test]
fn the_binding_carries_what_the_bytes_mean() {
    let asset = asset(true);
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");
    let region = &circuit.regions()[0];

    let mut registry = JitRegistry::new();
    registry.insert(
        IntrinsicKey {
            name: region.name.clone(),
            revision: region.revision,
            digest: region.digest,
        },
        Box::new(ToggleLowering),
    );
    let lowered = JitProgram::compile_with(&circuit, &registry).expect("compiles");

    let binding = &lowered.lane_state_regions()[0];
    assert_eq!(binding.name, "test.toggle");
    assert_eq!(binding.revision, region.revision);
    assert_eq!(binding.digest, region.digest);
    assert_eq!(binding.words, crdf_circuit::lane_state::words_for(1));
}
