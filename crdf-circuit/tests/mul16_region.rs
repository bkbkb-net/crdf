//! What a real multiply actually costs the JIT, and whether declaring
//! it an intrinsic is even possible.
//!
//! Both questions had answers I had assumed rather than measured, and
//! both assumptions were wrong in the same direction.

#![cfg(feature = "jit")]

use crdf_circuit::compile::{CompiledCircuit, flatten};
use crdf_circuit::eval::CircuitState;
use crdf_circuit::jit::JitProgram;
use crdf_circuit::model::{CircuitAsset, IntrinsicDecl, PortDirection};
use crdf_circuit::stdlib::Stdlib;
use std::time::Instant;

/// The standard 16x16 multiply, declared substitutable.
fn mul16_asset() -> (CircuitAsset, usize) {
    let stdlib = Stdlib::build();
    let mut library = stdlib.library;
    let id = stdlib.mul16;

    let module = library.get(id).expect("mul16 is in the library").clone();
    let inputs: Vec<_> = module
        .ports
        .iter()
        .filter(|p| p.direction == PortDirection::Input)
        .map(|p| p.id)
        .collect();
    let outputs: Vec<_> = module
        .ports
        .iter()
        .filter(|p| p.direction == PortDirection::Output)
        .map(|p| p.id)
        .collect();
    let gates = flatten(id, &library).expect("flattens").gate_count();

    let mut declared = module;
    declared.intrinsic = Some(IntrinsicDecl {
        name: "std.mul16".into(),
        revision: 1,
        inputs,
        outputs,
    });
    library.insert(declared);

    (CircuitAsset::new(id, 1, library), gates)
}

/// **A multiply can be claimed, and its one register is a constant.**
///
/// The first version of the eligibility rule refused any region holding
/// a register at all, on the grounds that skipping its gates would
/// leave a `d` pin undriven. `std.mul16` holds exactly one register and
/// it holds *itself* — which is how a constant is written in a world
/// with no constant sources — so its `d` comes from its own `q` and
/// skipping the gates loses nothing. Refusing it would have ruled out
/// the one thing worth accelerating.
#[test]
fn a_multiply_is_eligible_because_its_only_state_is_a_constant() {
    let (asset, _) = mul16_asset();
    let flat = flatten(asset.root, &asset.library).expect("flattens");

    let regs = flat.regs();
    assert_eq!(regs.len(), 1, "one register in a 16x16 multiply");
    assert_eq!(regs[0].d, regs[0].q, "and it holds itself");
    assert!(flat.intrinsics()[0].has_state, "reported honestly");

    let circuit = CompiledCircuit::compile(&asset).expect("compiles");
    assert_eq!(
        circuit.regions().len(),
        1,
        "and is offered for substitution anyway"
    );
}

/// **What the JIT actually executes for a multiply.**
///
/// I had been costing a substitution against the flat NAND count, and
/// the JIT does not execute NANDs. Folding turns the canonical 9-NAND
/// full adder into one op that emits five bitwise instructions, so a
/// multiply built from 240 adders and 256 two-NAND ANDs is nowhere near
/// 2,672 anythings by the time it reaches the code generator.
///
/// The gap matters because the whole question — is it worth calling out
/// to native arithmetic — is decided by this number against the 231 ns
/// a transpose pair costs. Printed, so the figure is visible; the
/// assertion only pins the shape.
#[test]
fn a_multiply_costs_what_the_jit_actually_runs() {
    let (asset, gates) = mul16_asset();
    let circuit = CompiledCircuit::compile(&asset).expect("compiles");
    let region = &circuit.regions()[0];

    println!("std.mul16: {gates} NAND gates");
    println!("  folded to {} instructions", region.fallback_len());
    assert!(
        region.fallback_len() < gates,
        "folding must reduce the count, or the fold is not happening"
    );

    let Ok(program) = JitProgram::compile(&circuit) else {
        println!("  (no JIT on this host)");
        return;
    };
    let mut state = CircuitState::new(&circuit);
    // A different value in every lane, so nothing collapses.
    for bit in 0..16 {
        let a = circuit
            .input_index(&format!("a.{bit}"))
            .expect("an input bit");
        let b = circuit
            .input_index(&format!("b.{bit}"))
            .expect("an input bit");
        for lane in 0..64 {
            state.set_level(&circuit, a, lane, (lane >> (bit % 6)) & 1 == 1);
            state.set_level(&circuit, b, lane, (lane >> ((bit + 3) % 6)) & 1 == 1);
        }
    }

    for _ in 0..200 {
        state.process_step_jit(&program);
    }
    const ROUNDS: u32 = 20_000;
    let started = Instant::now();
    for _ in 0..ROUNDS {
        state.process_step_jit(&program);
    }
    let per_tick = started.elapsed().as_nanos() as f64 / f64::from(ROUNDS);
    println!("  jit: {per_tick:.0} ns per tick, 64 lanes");

    // The interpreter has no optimiser of its own, so an instruction it
    // does not have to dispatch is an instruction saved. This is where
    // folding pays; see the note in the test below.
    let mut state = CircuitState::new(&circuit);
    for _ in 0..200 {
        state.process_step(&circuit);
    }
    let started = Instant::now();
    for _ in 0..ROUNDS {
        state.process_step(&circuit);
    }
    let interp = started.elapsed().as_nanos() as f64 / f64::from(ROUNDS);
    println!("  interpreter: {interp:.0} ns per tick, 64 lanes");
    println!("  a transpose pair costs 231 ns, so substitution wins above that");

    if cfg!(debug_assertions) {
        println!("  (unoptimised build -- figure is not the one to quote)");
    }
}
