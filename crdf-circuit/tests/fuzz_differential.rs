//! Differential fuzzing: randomly generated circuits are evaluated
//! through every backend and compared bit-for-bit.
//!
//! This is the structural defence against optimiser bugs: the folding
//! pass and the JIT are exactly the components most likely to break
//! subtly, so every random circuit is checked
//!
//! - unfolded reference simulator (`FlatSimulator`, never optimised)
//! - vs the folded bit-sliced interpreter
//! - vs the folded Cranelift JIT (when the feature is on)
//!
//! over hundreds of random input steps. The generator deliberately
//! plants full-adder and XOR patterns — sometimes with an extra tap on
//! an interior net, which must *prevent* folding without changing
//! behaviour — because pattern boundaries are where matcher bugs live.
//! Circuits are valid by construction (gate inputs reference earlier
//! nets or register outputs only), so every seed exercises the
//! evaluators rather than the error paths. Seeds are fixed: failures
//! reproduce deterministically.

use crdf_circuit::compile::{CompiledCircuit, flatten};
use crdf_circuit::eval::CircuitState;
use crdf_circuit::interp::FlatSimulator;
use crdf_circuit::model::{CellId, CircuitAsset, Endpoint, ModuleBuilder, ModuleLibrary};

/// xorshift64*: deterministic, dependency-free.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

/// Builds the canonical 9-NAND full adder inline; returns
/// (sum, cout, interior endpoints).
fn plant_full_adder(
    m: &mut ModuleBuilder,
    a: Endpoint,
    b: Endpoint,
    cin: Endpoint,
) -> (Endpoint, Endpoint, Vec<Endpoint>) {
    let nand = |m: &mut ModuleBuilder, x: Endpoint, y: Endpoint| -> CellId {
        let gate = m.nand();
        m.wire(x, Endpoint::nand_a(gate));
        m.wire(y, Endpoint::nand_b(gate));
        gate
    };
    let n1 = nand(m, a, b);
    let n2 = nand(m, a, Endpoint::nand_y(n1));
    let n3 = nand(m, b, Endpoint::nand_y(n1));
    let n4 = nand(m, Endpoint::nand_y(n2), Endpoint::nand_y(n3));
    let n5 = nand(m, Endpoint::nand_y(n4), cin);
    let n6 = nand(m, Endpoint::nand_y(n4), Endpoint::nand_y(n5));
    let n7 = nand(m, cin, Endpoint::nand_y(n5));
    let n8 = nand(m, Endpoint::nand_y(n6), Endpoint::nand_y(n7));
    let n9 = nand(m, Endpoint::nand_y(n5), Endpoint::nand_y(n1));
    let interiors = [n1, n2, n3, n4, n5, n6, n7]
        .into_iter()
        .map(Endpoint::nand_y)
        .collect();
    (Endpoint::nand_y(n8), Endpoint::nand_y(n9), interiors)
}

/// Builds the canonical 4-NAND XOR inline; returns
/// (out, interior endpoints).
fn plant_xor(m: &mut ModuleBuilder, a: Endpoint, b: Endpoint) -> (Endpoint, Vec<Endpoint>) {
    let nand = |m: &mut ModuleBuilder, x: Endpoint, y: Endpoint| -> CellId {
        let gate = m.nand();
        m.wire(x, Endpoint::nand_a(gate));
        m.wire(y, Endpoint::nand_b(gate));
        gate
    };
    let n1 = nand(m, a, b);
    let n2 = nand(m, a, Endpoint::nand_y(n1));
    let n3 = nand(m, b, Endpoint::nand_y(n1));
    let n4 = nand(m, Endpoint::nand_y(n2), Endpoint::nand_y(n3));
    let interiors = [n1, n2, n3].into_iter().map(Endpoint::nand_y).collect();
    (Endpoint::nand_y(n4), interiors)
}

const INPUTS: usize = 17;
const OUTPUTS: usize = 8;

/// Generates a random, valid-by-construction circuit. Port names are
/// deliberately arbitrary: the fuzzer exercises the generic engine,
/// proving nothing depends on any adapter's naming.
fn random_asset(seed: u64) -> CircuitAsset {
    let mut rng = Rng::new(seed);
    let mut m = ModuleBuilder::new(format!("fuzz-{seed}"));

    let mut sources: Vec<Endpoint> = Vec::new();
    for bit in 0..INPUTS {
        let port = m.input(format!("x{bit}"));
        sources.push(Endpoint::module(port));
    }

    let regs: Vec<CellId> = (0..2 + rng.below(6))
        .map(|_| m.reg(rng.chance(50)))
        .collect();
    for reg in &regs {
        sources.push(Endpoint::reg_q(*reg));
    }

    let pick = |rng: &mut Rng, sources: &[Endpoint]| sources[rng.below(sources.len())];

    let op_count = 20 + rng.below(60);
    for _ in 0..op_count {
        match rng.below(10) {
            // Plain NAND soup.
            0..=5 => {
                let gate = m.nand();
                m.wire(pick(&mut rng, &sources), Endpoint::nand_a(gate));
                m.wire(pick(&mut rng, &sources), Endpoint::nand_b(gate));
                sources.push(Endpoint::nand_y(gate));
            }
            // Planted full adder, occasionally with a tapped interior
            // (which must block folding, not break behaviour).
            6 | 7 => {
                let a = pick(&mut rng, &sources);
                let b = pick(&mut rng, &sources);
                let cin = pick(&mut rng, &sources);
                let (sum, cout, interiors) = plant_full_adder(&mut m, a, b, cin);
                if rng.chance(25) {
                    let tap = m.nand();
                    m.wire(interiors[rng.below(interiors.len())], Endpoint::nand_a(tap));
                    m.wire(pick(&mut rng, &sources), Endpoint::nand_b(tap));
                    sources.push(Endpoint::nand_y(tap));
                }
                sources.push(sum);
                sources.push(cout);
            }
            // Planted XOR, same optional interior tap.
            _ => {
                let a = pick(&mut rng, &sources);
                let b = pick(&mut rng, &sources);
                let (out, interiors) = plant_xor(&mut m, a, b);
                if rng.chance(25) {
                    let tap = m.nand();
                    m.wire(interiors[rng.below(interiors.len())], Endpoint::nand_a(tap));
                    m.wire(pick(&mut rng, &sources), Endpoint::nand_b(tap));
                    sources.push(Endpoint::nand_y(tap));
                }
                sources.push(out);
            }
        }
    }

    // Close every register loop and drive the outputs.
    for reg in &regs {
        m.wire(pick(&mut rng, &sources), Endpoint::reg_d(*reg));
    }
    for bit in 0..OUTPUTS {
        let port = m.output(format!("y{bit}"));
        m.wire(pick(&mut rng, &sources), Endpoint::module(port));
    }

    let root = m.id();
    let mut library = ModuleLibrary::new();
    library.insert(m.finish());
    CircuitAsset::new(root, 1 + rng.below(3) as u8, library)
}

#[test]
fn random_circuits_agree_across_all_backends() {
    for seed in 1..=40_u64 {
        let asset = random_asset(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let circuit = CompiledCircuit::compile(&asset)
            .unwrap_or_else(|e| panic!("seed {seed}: generator produced invalid circuit: {e}"));
        let flat = flatten(asset.root, &asset.library).unwrap();

        let inputs: Vec<usize> = (0..INPUTS)
            .map(|bit| circuit.input_index(&format!("x{bit}")).unwrap())
            .collect();
        let outputs: Vec<usize> = (0..OUTPUTS)
            .map(|bit| circuit.output_index(&format!("y{bit}")).unwrap())
            .collect();

        let mut oracle = FlatSimulator::new(&flat);
        let mut interp_state = CircuitState::new(&circuit);

        #[cfg(feature = "jit")]
        let jit = crdf_circuit::jit::JitProgram::compile(&circuit)
            .unwrap_or_else(|e| panic!("seed {seed}: jit compile failed: {e}"));
        #[cfg(feature = "jit")]
        let mut jit_state = CircuitState::new(&circuit);

        let mut rng = Rng::new(seed ^ 0xDEAD_BEEF);
        for step in 0..200 {
            let word = rng.next();
            // A random subset of inputs additionally receives a
            // one-step pulse — including inputs whose level is already
            // high, the exact case the JIT fold must not corrupt.
            let pulse_word = if rng.chance(30) { rng.next() } else { 0 };
            for (bit, input) in inputs.iter().enumerate() {
                let value = (word >> bit) & 1 == 1;
                let pulsed = (pulse_word >> bit) & 1 == 1;
                // Drive lane 0 (compared against the oracle) and lane
                // 63 (exercises the top lane) identically.
                for lane in [0, 63] {
                    interp_state.set_level(&circuit, *input, lane, value);
                    #[cfg(feature = "jit")]
                    jit_state.set_level(&circuit, *input, lane, value);
                    if pulsed {
                        interp_state.pulse(&circuit, *input, lane);
                        #[cfg(feature = "jit")]
                        jit_state.pulse(&circuit, *input, lane);
                    }
                }
            }

            interp_state.process_step(&circuit);
            // The oracle ticks once per circuit tick: tick 0 sees
            // level | pulse, later ticks see the levels only.
            for tick in 0..asset.ticks_per_step {
                for bit in 0..INPUTS {
                    let value = (word >> bit) & 1 == 1;
                    let pulsed = tick == 0 && (pulse_word >> bit) & 1 == 1;
                    assert!(oracle.set_input(&format!("x{bit}"), value || pulsed));
                }
                oracle.tick();
            }

            for (bit, output) in outputs.iter().enumerate() {
                let expected = oracle.output(&format!("y{bit}")).unwrap();
                assert_eq!(
                    interp_state.output_bit(*output, 0),
                    expected,
                    "seed {seed} step {step} y{bit}: interpreter diverged from oracle"
                );
                assert_eq!(
                    interp_state.output_bit(*output, 63),
                    expected,
                    "seed {seed} step {step} y{bit}: lane 63 diverged from lane 0"
                );
            }

            #[cfg(feature = "jit")]
            {
                jit_state.process_step_jit(&jit);
                for output in &outputs {
                    assert_eq!(
                        interp_state.output_word(*output),
                        jit_state.output_word(*output),
                        "seed {seed} step {step}: JIT diverged from interpreter"
                    );
                }
            }
        }
    }
}

#[test]
fn tapped_interiors_block_folding_but_preserve_behaviour() {
    // Directed version of the fuzz property: a full adder with a
    // tapped interior must not fold, and both the fused and unfused
    // compilations of the same logical function must agree.
    let mut m = ModuleBuilder::new("tapped-fa");
    let a = m.input("a");
    let b = m.input("b");
    let cin = m.input("cin");
    let out_sum = m.output("sum");
    let out_cout = m.output("cout");
    let out_tap = m.output("tap");
    let (sum, cout, interiors) = plant_full_adder(
        &mut m,
        Endpoint::module(a),
        Endpoint::module(b),
        Endpoint::module(cin),
    );
    m.wire(sum, Endpoint::module(out_sum));
    m.wire(cout, Endpoint::module(out_cout));
    // Tap n4 (= a xor b) straight to an output.
    m.wire(interiors[3], Endpoint::module(out_tap));
    let root = m.id();
    let mut library = ModuleLibrary::new();
    library.insert(m.finish());
    let asset = CircuitAsset::new(root, 1, library);

    let circuit = CompiledCircuit::compile(&asset).unwrap();
    assert_eq!(
        circuit.folded_adder_count(),
        0,
        "tapped adder must not fold"
    );

    let inputs = ["a", "b", "cin"].map(|name| circuit.input_index(name).unwrap());
    let outputs = ["sum", "cout", "tap"].map(|name| circuit.output_index(name).unwrap());
    let mut state = CircuitState::new(&circuit);
    for bits in 0..8_u8 {
        for (index, input) in inputs.iter().enumerate() {
            state.set_level(&circuit, *input, 0, (bits >> index) & 1 == 1);
        }
        state.process_step(&circuit);
        let (a, b, cin) = (bits & 1, (bits >> 1) & 1, (bits >> 2) & 1);
        let total = a + b + cin;
        assert_eq!(
            state.output_bit(outputs[0], 0),
            total & 1 == 1,
            "sum {bits}"
        );
        assert_eq!(state.output_bit(outputs[1], 0), total >= 2, "cout {bits}");
        assert_eq!(state.output_bit(outputs[2], 0), a ^ b == 1, "tap {bits}");
    }
}

#[cfg(feature = "jit")]
#[test]
fn pulsing_an_already_high_level_must_not_clear_it() {
    // Regression: the JIT path folds pulses into the level word and
    // must only remove the bits the pulse actually turned on.
    for ticks in [1_u8, 2] {
        let mut m = ModuleBuilder::new("pulse-high");
        let p = m.input("p");
        let y = m.output("y");
        m.wire(Endpoint::module(p), Endpoint::module(y));
        let root = m.id();
        let mut library = ModuleLibrary::new();
        library.insert(m.finish());
        let asset = CircuitAsset::new(root, ticks, library);
        let circuit = CompiledCircuit::compile(&asset).unwrap();
        let jit = crdf_circuit::jit::JitProgram::compile(&circuit).unwrap();
        let input = circuit.input_index("p").unwrap();
        let output = circuit.output_index("y").unwrap();

        let mut interp = CircuitState::new(&circuit);
        let mut jitted = CircuitState::new(&circuit);
        for state in [&mut interp, &mut jitted] {
            state.set_level(&circuit, input, 0, true);
            state.pulse(&circuit, input, 0);
        }

        for step in 0..3 {
            interp.process_step(&circuit);
            jitted.process_step_jit(&jit);
            assert!(
                interp.output_bit(output, 0),
                "T={ticks} step {step}: interpreter lost the level"
            );
            assert_eq!(
                interp.output_word(output),
                jitted.output_word(output),
                "T={ticks} step {step}: JIT diverged"
            );
        }
    }
}

/// A lowered delay line must be indistinguishable from the registers it
/// replaced.
///
/// This is the test the whole lowering rests on: `FlatSimulator` never
/// optimises anything, so if the ring buffer and the literal shift chain
/// ever disagree, the recognition is wrong and must be narrowed rather
/// than patched.
#[test]
fn a_lowered_delay_line_matches_the_unoptimised_simulator() {
    use crdf_circuit::interp::FlatSimulator;

    for stages in [crdf_circuit::delay::MIN_CHAIN, 191, 256] {
        let mut m = ModuleBuilder::new("line");
        let input = m.input("in");
        let out = m.output("out");
        // A read partway along, so the bank has to publish an interior
        // tap as well as the line's own end.
        let mid_out = m.output("mid");
        let mut at = Endpoint::module(input);
        let mut mid = None;
        for index in 0..stages {
            let reg = m.reg(false);
            m.wire(at, Endpoint::reg_d(reg));
            at = Endpoint::reg_q(reg);
            if index + 1 == stages / 2 {
                mid = Some(at);
            }
        }
        m.wire(at, Endpoint::module(out));
        m.wire(mid.expect("a midpoint"), Endpoint::module(mid_out));

        let root = m.id();
        let mut library = ModuleLibrary::new();
        library.insert(m.finish());
        let asset = CircuitAsset::new(root, 1, library);

        let flat = crdf_circuit::compile::flatten(root, &asset.library).unwrap();
        let recognised = crdf_circuit::delay::recognise(&flat);
        assert_eq!(
            recognised.banks.len(),
            1,
            "{stages} stages should lower to one bank"
        );

        let circuit = CompiledCircuit::compile(&asset).unwrap();
        let mut reference = FlatSimulator::new(&flat);
        let mut lowered = CircuitState::new(&circuit);
        let in_index = circuit.input_index("in").unwrap();
        let out_index = circuit.output_index("out").unwrap();
        let mid_index = circuit.output_index("mid").unwrap();

        // A pattern long enough to fill the line several times over, and
        // irregular enough that an off-by-one in the ring shows up.
        let mut bit = 0xACE1_u16;
        for step in 0..(stages * 4 + 17) {
            bit = (bit >> 1) ^ (0xB400 * (bit & 1));
            let value = bit & 1 == 1;

            assert!(
                reference.set_input("in", value),
                "the reference knows the port"
            );
            reference.tick();
            lowered.set_level(&circuit, in_index, 0, value);
            lowered.process_step(&circuit);

            assert_eq!(
                lowered.output_bit(out_index, 0),
                reference.output("out").unwrap(),
                "{stages} stages, step {step}: the line's output diverged"
            );
            assert_eq!(
                lowered.output_bit(mid_index, 0),
                reference.output("mid").unwrap(),
                "{stages} stages, step {step}: the interior tap diverged"
            );
        }
    }
}

/// The JIT must refuse a lowered circuit rather than run it without its
/// delay state, which would be wrong output and no error.
#[cfg(feature = "jit")]
#[test]
fn the_jit_refuses_a_circuit_it_cannot_evaluate() {
    let mut m = ModuleBuilder::new("line");
    let input = m.input("in");
    let out = m.output("out");
    let mut at = Endpoint::module(input);
    for _ in 0..crdf_circuit::delay::MIN_CHAIN {
        let reg = m.reg(false);
        m.wire(at, Endpoint::reg_d(reg));
        at = Endpoint::reg_q(reg);
    }
    m.wire(at, Endpoint::module(out));
    let root = m.id();
    let mut library = ModuleLibrary::new();
    library.insert(m.finish());
    let circuit = CompiledCircuit::compile(&CircuitAsset::new(root, 1, library)).unwrap();

    assert!(matches!(
        crdf_circuit::jit::JitProgram::compile(&circuit),
        Err(crdf_circuit::jit::JitError::UnsupportedDelayBanks(1))
    ));
}

/// A delay long enough to be useful must reach the recogniser.
///
/// The register cap used to be checked while flattening, so a line of
/// this length was rejected before the pass that collapses it into a
/// ring ever saw it — lowering bought nothing for exactly the lengths it
/// was for. The compile budget and the runtime state budget are now
/// separate questions.
#[test]
fn a_long_line_compiles_and_lowers() {
    // 4,800 stages of one bit. A 16-bit line is sixteen of these,
    // which is why the compile budget has to be in the hundreds of
    // thousands rather than tens.
    const STAGES: usize = 4_800;

    let mut m = ModuleBuilder::new("echo");
    let input = m.input("in");
    let out = m.output("out");
    let mut at = Endpoint::module(input);
    for _ in 0..STAGES {
        let reg = m.reg(false);
        m.wire(at, Endpoint::reg_d(reg));
        at = Endpoint::reg_q(reg);
    }
    m.wire(at, Endpoint::module(out));
    let root = m.id();
    let mut library = ModuleLibrary::new();
    library.insert(m.finish());

    let circuit = CompiledCircuit::compile(&CircuitAsset::new(root, 1, library))
        .expect("a hundred milliseconds of line is not an unreasonable circuit");
    assert_eq!(
        circuit.reg_count(),
        0,
        "the whole line should have become a bank"
    );

    // And it still delays by exactly what was asked for.
    let mut state = CircuitState::new(&circuit);
    let in_index = circuit.input_index("in").unwrap();
    let out_index = circuit.output_index("out").unwrap();
    state.set_level(&circuit, in_index, 0, true);
    for step in 0..STAGES {
        state.process_step(&circuit);
        assert!(
            !state.output_bit(out_index, 0),
            "step {step}: the line gave its input back early"
        );
    }
    state.process_step(&circuit);
    assert!(
        state.output_bit(out_index, 0),
        "and arrives on the tick it was due"
    );
}
