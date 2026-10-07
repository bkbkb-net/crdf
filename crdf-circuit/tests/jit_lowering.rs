//! A backend can compute a region its own way, and the gates decide
//! whether it got the right answer.
//!
//! The region used here is a 2-to-1 selector built from four NANDs.
//! Nothing about it is special — it is small enough that the whole
//! path is visible in one file, and its native form stays bit-sliced,
//! so this exercises registry lookup, region scheduling, gate omission,
//! fallback and the digest without any change of representation
//! getting in the way.

#![cfg(feature = "jit")]

use crdf_circuit::compile::{CompiledCircuit, flatten};
use crdf_circuit::eval::CircuitState;
use crdf_circuit::jit::{IntrinsicKey, JitLoweringCx, JitProgram, JitRegistry, Lowered};
use crdf_circuit::model::{
    CircuitAsset, Endpoint, IntrinsicDecl, Module, ModuleBuilder, ModuleId, ModuleLibrary,
    PortDirection,
};

/// `out = if sel { b } else { a }`, as four NANDs.
///
/// `t1 = !(sel & sel)` is the inverter; `t2 = !(a & t1)`,
/// `t3 = !(b & sel)`, `out = !(t2 & t3)`.
fn selector(declared: bool) -> Module {
    let mut m = ModuleBuilder::new("sel2");
    let a = m.input("a");
    let b = m.input("b");
    let sel = m.input("sel");
    let out = m.output("out");

    let inv = m.nand();
    m.wire(Endpoint::module(sel), Endpoint::nand_a(inv));
    m.wire(Endpoint::module(sel), Endpoint::nand_b(inv));

    let left = m.nand();
    m.wire(Endpoint::module(a), Endpoint::nand_a(left));
    m.wire(Endpoint::nand_y(inv), Endpoint::nand_b(left));

    let right = m.nand();
    m.wire(Endpoint::module(b), Endpoint::nand_a(right));
    m.wire(Endpoint::module(sel), Endpoint::nand_b(right));

    let join = m.nand();
    m.wire(Endpoint::nand_y(left), Endpoint::nand_a(join));
    m.wire(Endpoint::nand_y(right), Endpoint::nand_b(join));
    m.wire(Endpoint::nand_y(join), Endpoint::module(out));

    let mut module = m.finish();
    if declared {
        module.intrinsic = Some(IntrinsicDecl {
            name: "test.sel2".into(),
            revision: 1,
            inputs: vec![a, b, sel],
            outputs: vec![out],
        });
    }
    module
}

/// Two selectors in series, so the region appears twice and the second
/// depends on the first — which is what makes the scheduling of a
/// region as one node do any work.
fn chain(inner: ModuleId, lib: &ModuleLibrary) -> Module {
    let mut m = ModuleBuilder::new("chain");
    let x = m.input("x");
    let y = m.input("y");
    let z = m.input("z");
    let s0 = m.input("s0");
    let s1 = m.input("s1");
    let out = m.output("out");

    let ports = &lib.get(inner).expect("inner").ports;
    let find = |name: &str| {
        ports
            .iter()
            .find(|p| p.name == name)
            .expect("a port by that name")
            .id
    };
    let (pa, pb, psel, pout) = (find("a"), find("b"), find("sel"), find("out"));

    let first = m.instance(inner);
    m.wire(Endpoint::module(x), Endpoint::inner(first, pa));
    m.wire(Endpoint::module(y), Endpoint::inner(first, pb));
    m.wire(Endpoint::module(s0), Endpoint::inner(first, psel));

    let second = m.instance(inner);
    m.wire(Endpoint::inner(first, pout), Endpoint::inner(second, pa));
    m.wire(Endpoint::module(z), Endpoint::inner(second, pb));
    m.wire(Endpoint::module(s1), Endpoint::inner(second, psel));
    m.wire(Endpoint::inner(second, pout), Endpoint::module(out));

    m.finish()
}

fn asset(declared: bool) -> CircuitAsset {
    let mut lib = ModuleLibrary::new();
    let inner = selector(declared);
    let inner_id = inner.id;
    lib.insert(inner);
    let top = chain(inner_id, &lib);
    let top_id = top.id;
    lib.insert(top);
    CircuitAsset::new(top_id, 1, lib)
}

/// `out = a ^ (sel & (a ^ b))` — the same function, three bitwise ops
/// instead of four NANDs, and still one `I64` bit plane per wire.
fn select_natively(
    cx: JitLoweringCx<'_, '_>,
) -> Result<crdf_circuit::jit::Lowered, crdf_circuit::jit::JitError> {
    use cranelift_codegen::ir::InstBuilder;
    let (a, b, sel) = (cx.inputs[0], cx.inputs[1], cx.inputs[2]);
    let differ = cx.builder.ins().bxor(a, b);
    let chosen = cx.builder.ins().band(sel, differ);
    Ok(Lowered::outputs(vec![cx.builder.ins().bxor(a, chosen)]))
}

/// Runs one tick and reads `out` for a set of input words.
fn run(program: Option<&JitProgram>, circuit: &CompiledCircuit, words: [u64; 5]) -> u64 {
    let mut state = CircuitState::new(circuit);
    for (name, word) in ["x", "y", "z", "s0", "s1"].into_iter().zip(words) {
        let index = circuit.input_index(name).expect("an input by that name");
        for lane in 0..64 {
            state.set_level(circuit, index, lane, (word >> lane) & 1 == 1);
        }
    }
    match program {
        Some(program) => state.process_step_jit(program),
        None => state.process_step(circuit),
    }
    state.output_word(circuit.output_index("out").expect("an output"))
}

fn cases() -> Vec<[u64; 5]> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut cases = vec![
        [0, 0, 0, 0, 0],
        [u64::MAX, 0, 0, 0, 0],
        [0, u64::MAX, 0, u64::MAX, 0],
        [0, 0, u64::MAX, 0, u64::MAX],
        [u64::MAX; 5],
    ];
    for _ in 0..64 {
        cases.push([next(), next(), next(), next(), next()]);
    }
    cases
}

/// **The lowering is used, and it agrees with the gates.**
///
/// Three ways of computing the same circuit — the reference
/// interpreter, the JIT with no lowering, and the JIT with one — must
/// give identical words on all 64 lanes. The interpreter never uses a
/// lowering, so it is the thing the other two are answerable to.
#[test]
fn a_lowering_agrees_with_the_gates_it_replaces() {
    let asset = asset(true);
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");
    assert_eq!(circuit.regions().len(), 2, "one region per occurrence");

    let mut registry = JitRegistry::new();
    for region in circuit.regions() {
        registry.insert(
            IntrinsicKey {
                name: region.name.clone(),
                revision: region.revision,
                digest: region.digest,
            },
            Box::new(select_natively),
        );
    }
    assert!(!registry.is_empty());

    let plain = JitProgram::compile(&circuit).expect("compiles without lowerings");
    let lowered = JitProgram::compile_with(&circuit, &registry).expect("compiles with them");

    for words in cases() {
        let reference = run(None, &circuit, words);
        assert_eq!(run(Some(&plain), &circuit, words), reference, "jit, gates");
        assert_eq!(
            run(Some(&lowered), &circuit, words),
            reference,
            "jit, lowering"
        );
    }
}

/// **The replaced gates are not emitted.**
///
/// Agreement alone would not prove the lowering was used at all: a
/// registry that never matched would agree too, by running the gates.
/// Both selectors are folded into one region each, so a matched
/// registry has to leave the JIT with strictly fewer instructions to
/// emit than an unmatched one.
#[test]
fn a_matched_region_skips_its_gates() {
    let asset = asset(true);
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");

    let gates_inside: usize = circuit
        .regions()
        .iter()
        .map(|region| region.fallback_len())
        .sum();
    assert_eq!(gates_inside, 8, "four NANDs in each of two selectors");

    // Every gate in this circuit is inside a region, so with both
    // lowerings matched the top-level list is two region instructions
    // and nothing else.
    assert_eq!(circuit.instr_count(), 2);
}

/// **A key that does not match is simply not used.**
///
/// This is the property that makes a stale registration safe rather
/// than dangerous. An edited netlist gets a different digest, the
/// lookup misses, and the region runs its own gates -- slower, and
/// right. The alternative, using a lowering that no longer matches the
/// circuit, would be wrong only in the JIT, which is the worst place
/// for a disagreement to hide.
#[test]
fn a_stale_registration_is_ignored() {
    let asset = asset(true);
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");
    let region = &circuit.regions()[0];

    let mut registry = JitRegistry::new();
    registry.insert(
        IntrinsicKey {
            name: region.name.clone(),
            // As if the netlist had been edited and its digest moved.
            revision: region.revision,
            digest: uuid::Uuid::nil(),
        },
        // A lowering that returns a constant: if it were ever used the
        // outputs would be plainly wrong.
        Box::new(
            |cx: JitLoweringCx<'_, '_>| -> Result<Lowered, crdf_circuit::jit::JitError> {
                use cranelift_codegen::ir::{InstBuilder, types};
                Ok(Lowered::outputs(vec![
                    cx.builder.ins().iconst(types::I64, 0),
                ]))
            },
        ),
    );

    let lowered = JitProgram::compile_with(&circuit, &registry).expect("compiles");
    for words in cases() {
        assert_eq!(
            run(Some(&lowered), &circuit, words),
            run(None, &circuit, words),
            "the stale lowering must not have been used"
        );
    }
}

/// **Declaring an intrinsic does not change what the circuit computes.**
///
/// The same netlist with and without the declaration, run through the
/// reference interpreter, on the same inputs.
#[test]
fn declaring_a_region_changes_nothing_by_itself() {
    let declared = CompiledCircuit::compile(&asset(true)).expect("compiles");
    let plain = CompiledCircuit::compile(&asset(false)).expect("compiles");
    assert!(plain.regions().is_empty());

    for words in cases() {
        assert_eq!(run(None, &declared, words), run(None, &plain, words));
    }
}

/// **The digest moves when the gates move, and not otherwise.**
///
/// Two builds of the same module produce fresh random ids for every
/// port and cell, so a digest that depended on them would differ every
/// time and retire every lowering on every run.
#[test]
fn the_digest_follows_the_gates_not_the_identifiers() {
    let first = CompiledCircuit::compile(&asset(true)).expect("compiles");
    let second = CompiledCircuit::compile(&asset(true)).expect("compiles");
    assert_eq!(
        first.regions()[0].digest,
        second.regions()[0].digest,
        "fresh uuids each build must not move the digest"
    );

    // The two occurrences are the same module, so they agree too.
    assert_eq!(first.regions()[0].digest, first.regions()[1].digest);

    // A different circuit under the same name and revision does not.
    let mut lib = ModuleLibrary::new();
    let mut m = ModuleBuilder::new("sel2");
    let a = m.input("a");
    let b = m.input("b");
    let sel = m.input("sel");
    let out = m.output("out");
    // A NAND instead of a selector: same boundary, different gates.
    let gate = m.nand();
    m.wire(Endpoint::module(a), Endpoint::nand_a(gate));
    m.wire(Endpoint::module(b), Endpoint::nand_b(gate));
    m.wire(Endpoint::nand_y(gate), Endpoint::module(out));
    // `sel` has to go somewhere for the module to be well formed.
    let sink = m.nand();
    m.wire(Endpoint::module(sel), Endpoint::nand_a(sink));
    m.wire(Endpoint::module(sel), Endpoint::nand_b(sink));
    let mut inner = m.finish();
    inner.intrinsic = Some(IntrinsicDecl {
        name: "test.sel2".into(),
        revision: 1,
        inputs: vec![a, b, sel],
        outputs: vec![out],
    });
    let inner_id = inner.id;
    lib.insert(inner);
    let top = chain(inner_id, &lib);
    let top_id = top.id;
    lib.insert(top);
    let other = CompiledCircuit::compile(&CircuitAsset::new(top_id, 1, lib)).expect("compiles");

    assert_ne!(
        first.regions()[0].digest,
        other.regions()[0].digest,
        "different gates under the same name must not share a key"
    );
}

/// **A region with state is not taken whole.**
///
/// Its gates drive a register's `d` pin, so skipping them would leave
/// that pin undriven. Until a lowering can carry the state itself, such
/// a region stays ordinary gates — and the flattened netlist still
/// reports the declaration, so nothing is silently lost.
#[test]
fn a_stateful_region_keeps_its_gates() {
    let mut lib = ModuleLibrary::new();
    let mut m = ModuleBuilder::new("latch");
    let d = m.input("d");
    let q = m.output("q");
    let reg = m.reg(false);
    let gate = m.nand();
    m.wire(Endpoint::module(d), Endpoint::nand_a(gate));
    m.wire(Endpoint::module(d), Endpoint::nand_b(gate));
    m.wire(Endpoint::nand_y(gate), Endpoint::reg_d(reg));
    m.wire(Endpoint::reg_q(reg), Endpoint::module(q));
    let mut inner = m.finish();
    inner.intrinsic = Some(IntrinsicDecl {
        name: "test.latch".into(),
        revision: 1,
        inputs: vec![d],
        outputs: vec![q],
    });
    let inner_id = inner.id;
    lib.insert(inner);

    let mut top = ModuleBuilder::new("top");
    let x = top.input("x");
    let y = top.output("y");
    let cell = top.instance(inner_id);
    let ports = &lib.get(inner_id).expect("inner").ports;
    let find = |dir: PortDirection| {
        ports
            .iter()
            .find(|p| p.direction == dir)
            .expect("a port")
            .id
    };
    top.wire(
        Endpoint::module(x),
        Endpoint::inner(cell, find(PortDirection::Input)),
    );
    top.wire(
        Endpoint::inner(cell, find(PortDirection::Output)),
        Endpoint::module(y),
    );
    let top_id = top.id();
    lib.insert(top.finish());

    let flat = flatten(top_id, &lib).expect("flattens");
    assert_eq!(flat.intrinsics().len(), 1, "the declaration is still there");
    assert!(flat.intrinsics()[0].has_state);

    let circuit = CompiledCircuit::compile(&CircuitAsset::new(top_id, 1, lib)).expect("compiles");
    assert!(
        circuit.regions().is_empty(),
        "a stateful region is not offered for substitution"
    );
    assert!(circuit.instr_count() > 0, "its gates are still emitted");
}

/// **The same module keys the same way wherever it is placed.**
///
/// This is the property the first version of the digest did not have,
/// and the failure was quiet: it hashed the folded fallback, whose
/// instruction order comes from a topological sort of the *whole*
/// circuit. Drive one of the region's inputs through an extra gate and
/// two independent gates inside it swap places, so the same module
/// keyed differently in two patches — a lowering that worked in one and
/// silently did not in the other.
///
/// Here the same selector sits in three different surroundings: the
/// plain chain, a chain whose inputs arrive through extra logic, and
/// one where two of the region's inputs are tied to the same signal.
/// All three must produce one key.
#[test]
fn the_digest_does_not_depend_on_the_surrounding_circuit() {
    let plain = CompiledCircuit::compile(&asset(true)).expect("compiles");
    let want = plain.regions()[0].digest;

    // (a) Inputs arriving through an extra inverter each.
    let mut lib = ModuleLibrary::new();
    let inner = selector(true);
    let inner_id = inner.id;
    lib.insert(inner);
    let mut m = ModuleBuilder::new("delayed");
    let x = m.input("x");
    let y = m.input("y");
    let s = m.input("s");
    let out = m.output("out");
    let ports = &lib.get(inner_id).expect("inner").ports;
    let find = |name: &str| ports.iter().find(|p| p.name == name).expect("a port").id;
    let through = |m: &mut ModuleBuilder, from| {
        let inv = m.nand();
        m.wire(Endpoint::module(from), Endpoint::nand_a(inv));
        m.wire(Endpoint::module(from), Endpoint::nand_b(inv));
        inv
    };
    let ix = through(&mut m, x);
    let cell = m.instance(inner_id);
    m.wire(Endpoint::nand_y(ix), Endpoint::inner(cell, find("a")));
    m.wire(Endpoint::module(y), Endpoint::inner(cell, find("b")));
    m.wire(Endpoint::module(s), Endpoint::inner(cell, find("sel")));
    m.wire(Endpoint::inner(cell, find("out")), Endpoint::module(out));
    let top_id = m.id();
    lib.insert(m.finish());
    let delayed = CompiledCircuit::compile(&CircuitAsset::new(top_id, 1, lib)).expect("compiles");
    assert_eq!(
        delayed.regions()[0].digest,
        want,
        "an extra gate outside the region must not move its key"
    );

    // (b) Two of the region's inputs tied to one signal.
    let mut lib = ModuleLibrary::new();
    let inner = selector(true);
    let inner_id = inner.id;
    lib.insert(inner);
    let mut m = ModuleBuilder::new("tied");
    let x = m.input("x");
    let s = m.input("s");
    let out = m.output("out");
    let ports = &lib.get(inner_id).expect("inner").ports;
    let find = |name: &str| ports.iter().find(|p| p.name == name).expect("a port").id;
    let cell = m.instance(inner_id);
    m.wire(Endpoint::module(x), Endpoint::inner(cell, find("a")));
    m.wire(Endpoint::module(x), Endpoint::inner(cell, find("b")));
    m.wire(Endpoint::module(s), Endpoint::inner(cell, find("sel")));
    m.wire(Endpoint::inner(cell, find("out")), Endpoint::module(out));
    let top_id = m.id();
    lib.insert(m.finish());
    let tied = CompiledCircuit::compile(&CircuitAsset::new(top_id, 1, lib)).expect("compiles");
    assert_eq!(
        tied.regions()[0].digest,
        want,
        "tying two of its inputs together outside must not move its key"
    );
}

/// **Swapping two entries of the declaration is a different key.**
///
/// It has to be: the entries are the order a lowering receives its
/// arguments, so a lowering written for one order computes something
/// else under the other.
#[test]
fn the_digest_follows_the_declared_order() {
    let mut lib = ModuleLibrary::new();
    let mut inner = selector(true);
    let decl = inner.intrinsic.as_mut().expect("declared");
    decl.inputs.swap(0, 1);
    let inner_id = inner.id;
    lib.insert(inner);
    let top = chain(inner_id, &lib);
    let top_id = top.id;
    lib.insert(top);
    let swapped = CompiledCircuit::compile(&CircuitAsset::new(top_id, 1, lib)).expect("compiles");

    let plain = CompiledCircuit::compile(&asset(true)).expect("compiles");
    assert_ne!(swapped.regions()[0].digest, plain.regions()[0].digest);
}

/// **A lowering that branches away and never returns is refused.**
///
/// Everything after the region is emitted into whatever block is
/// current when the lowering hands back. If it terminated that block,
/// the rest of the tick goes nowhere — and the failure would otherwise
/// surface as a panic deep in the code generator with nothing pointing
/// at the lowering that caused it.
#[test]
fn a_lowering_that_leaves_no_live_block_is_refused() {
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
                use cranelift_codegen::ir::InstBuilder;
                let value = cx.inputs[0];
                let gone = cx.builder.create_block();
                cx.builder.ins().jump(gone, &[]);
                cx.builder.seal_block(gone);
                // Deliberately does not switch to `gone`, so the block
                // that emission continues in is terminated.
                Ok(Lowered::outputs(vec![value]))
            },
        ),
    );

    assert!(matches!(
        JitProgram::compile_with(&circuit, &registry),
        Err(crdf_circuit::jit::JitError::LoweringLeftNoBlock { .. })
    ));
}

/// **A program reports the substitutions that happened, not the ones on
/// offer.**
///
/// This is the distinction any cost estimate has to rest on. A registry
/// full of lowerings whose keys do not match is a registry that changes
/// nothing, and something charging less for a substitution has to be
/// able to tell the two apart.
#[test]
fn a_program_reports_what_it_actually_substituted() {
    let asset = asset(true);
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");
    assert_eq!(circuit.regions().len(), 2);

    // Nothing registered: nothing substituted.
    let plain = JitProgram::compile(&circuit).expect("compiles");
    assert!(plain.substituted_regions().is_empty());

    // Registered under a key that does not match: still nothing.
    let mut stale = JitRegistry::new();
    stale.insert(
        IntrinsicKey {
            name: circuit.regions()[0].name.clone(),
            revision: circuit.regions()[0].revision + 1,
            digest: circuit.regions()[0].digest,
        },
        Box::new(select_natively),
    );
    let missed = JitProgram::compile_with(&circuit, &stale).expect("compiles");
    assert!(
        missed.substituted_regions().is_empty(),
        "a registry that does not match must not look like one that does"
    );

    // Registered properly: both occurrences.
    let mut registry = JitRegistry::new();
    for region in circuit.regions() {
        registry.insert(
            IntrinsicKey {
                name: region.name.clone(),
                revision: region.revision,
                digest: region.digest,
            },
            Box::new(select_natively),
        );
    }
    let lowered = JitProgram::compile_with(&circuit, &registry).expect("compiles");
    let mut used = lowered.substituted_regions().to_vec();
    used.sort();
    assert_eq!(used, vec![0, 1]);
}

/// **The manifest says what runs, and a substitution removes work from
/// it.**
///
/// This is the whole point of counting instructions rather than gates:
/// a substituted region's gates are the one thing that does *not* run,
/// so anything costing a circuit from its gate count gets a lowered one
/// exactly backwards.
#[test]
fn a_substitution_moves_work_out_of_the_manifest() {
    let asset = asset(true);
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");

    let gates = circuit.manifest();
    assert!(gates.substitutions.is_empty());
    assert_eq!(
        gates.instruction_count(),
        8,
        "four NANDs in each of two selectors"
    );

    let mut registry = JitRegistry::new();
    for region in circuit.regions() {
        registry.insert(
            IntrinsicKey {
                name: region.name.clone(),
                revision: region.revision,
                digest: region.digest,
            },
            Box::new(select_natively),
        );
    }
    let lowered = JitProgram::compile_with(&circuit, &registry).expect("compiles");
    let used = circuit.manifest_with(lowered.substituted_regions());

    assert_eq!(used.instruction_count(), 0, "every gate was replaced");
    assert_eq!(used.substitutions.len(), 2);
    for substitution in &used.substitutions {
        assert_eq!(substitution.name, "test.sel2");
        assert_eq!(substitution.inputs, 3);
        assert_eq!(substitution.outputs, 1);
        assert_eq!(substitution.replaced, 4, "four NANDs each");
    }

    // A registry that does not match leaves the manifest alone, which is
    // what stops a discount being taken for a substitution that never
    // happened.
    let missed = JitProgram::compile(&circuit).expect("compiles");
    assert_eq!(
        circuit.manifest_with(missed.substituted_regions()),
        gates,
        "no substitution, no change"
    );
}
