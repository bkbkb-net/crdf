//! Generic engine invariants belong to `crdf-circuit`, not to a
//! downstream adapter. These tests intentionally use ordinary
//! port names and exercise validation, register semantics and stdlib
//! truth tables through the scalar reference interpreter.

use crdf_circuit::compile::{CompileError, flatten};
use crdf_circuit::interp::FlatSimulator;
use crdf_circuit::model::{
    Endpoint, MAX_HIERARCHY_DEPTH, Module, ModuleBuilder, ModuleId, ModuleLibrary,
};
use crdf_circuit::stdlib::Stdlib;

fn library(modules: impl IntoIterator<Item = Module>) -> ModuleLibrary {
    let mut library = ModuleLibrary::new();
    for module in modules {
        library.insert(module);
    }
    library
}

#[test]
fn combinational_cycles_are_rejected_and_registered_cycles_run() {
    let mut module = ModuleBuilder::new("cycle");
    let output = module.output("y");
    let gate = module.nand();
    module.wire(Endpoint::nand_y(gate), Endpoint::nand_a(gate));
    module.wire(Endpoint::nand_y(gate), Endpoint::nand_b(gate));
    module.wire(Endpoint::nand_y(gate), Endpoint::module(output));
    let root = module.id();
    assert!(matches!(
        flatten(root, &library([module.finish()])),
        Err(CompileError::CombinationalCycle { .. })
    ));

    let mut module = ModuleBuilder::new("registered-cycle");
    let output = module.output("y");
    let reg = module.reg(false);
    let gate = module.not_gate(Endpoint::reg_q(reg));
    module.wire(Endpoint::nand_y(gate), Endpoint::reg_d(reg));
    module.wire(Endpoint::reg_q(reg), Endpoint::module(output));
    let root = module.id();
    let flat = flatten(root, &library([module.finish()])).unwrap();
    let mut sim = FlatSimulator::new(&flat);
    let values: Vec<bool> = (0..4)
        .map(|_| {
            sim.tick();
            sim.output("y").unwrap()
        })
        .collect();
    assert_eq!(values, [false, true, false, true]);
}

#[test]
fn invalid_drivers_and_wire_directions_are_rejected() {
    let mut module = ModuleBuilder::new("undriven");
    let output = module.output("y");
    let gate = module.nand();
    module.wire(Endpoint::nand_y(gate), Endpoint::module(output));
    let root = module.id();
    assert!(matches!(
        flatten(root, &library([module.finish()])),
        Err(CompileError::UndrivenInput { .. })
    ));

    let mut module = ModuleBuilder::new("two-drivers");
    let output = module.output("y");
    let one = module.constant(true);
    let zero = module.constant(false);
    module.wire(Endpoint::reg_q(one), Endpoint::module(output));
    module.wire(Endpoint::reg_q(zero), Endpoint::module(output));
    let root = module.id();
    assert!(matches!(
        flatten(root, &library([module.finish()])),
        Err(CompileError::MultipleTopDrivers { .. } | CompileError::MultipleDrivers { .. })
    ));

    let mut module = ModuleBuilder::new("bad-direction");
    let one = module.constant(true);
    let gate = module.nand();
    module.wire(Endpoint::nand_y(gate), Endpoint::reg_q(one));
    let root = module.id();
    assert!(matches!(
        flatten(root, &library([module.finish()])),
        Err(CompileError::WireDirection { .. })
    ));
}

#[test]
fn recursion_and_hierarchy_limits_are_enforced() {
    let mut recursive = ModuleBuilder::new("recursive");
    let recursive_id = recursive.id();
    recursive.instance(recursive_id);
    assert!(matches!(
        flatten(recursive_id, &library([recursive.finish()])),
        Err(CompileError::DepthLimitExceeded { .. })
    ));

    let mut modules = Vec::new();
    let mut leaf = ModuleBuilder::new("leaf");
    leaf.constant(false);
    let mut previous = leaf.id();
    modules.push(leaf.finish());
    let mut at_limit = previous;
    for depth in 0..=MAX_HIERARCHY_DEPTH {
        let mut module = ModuleBuilder::new(format!("chain{depth}"));
        module.instance(previous);
        previous = module.id();
        modules.push(module.finish());
        if depth == MAX_HIERARCHY_DEPTH - 1 {
            at_limit = previous;
        }
    }
    let library = library(modules);
    assert!(flatten(at_limit, &library).is_ok());
    assert!(matches!(
        flatten(previous, &library),
        Err(CompileError::DepthLimitExceeded { .. })
    ));
}

#[test]
fn registers_latch_simultaneously() {
    let mut module = ModuleBuilder::new("rotate");
    let a = module.output("a");
    let b = module.output("b");
    let r0 = module.reg(true);
    let r1 = module.reg(false);
    module.wire(Endpoint::reg_q(r1), Endpoint::reg_d(r0));
    module.wire(Endpoint::reg_q(r0), Endpoint::reg_d(r1));
    module.wire(Endpoint::reg_q(r0), Endpoint::module(a));
    module.wire(Endpoint::reg_q(r1), Endpoint::module(b));
    let root = module.id();
    let flat = flatten(root, &library([module.finish()])).unwrap();
    let mut sim = FlatSimulator::new(&flat);
    let values: Vec<(bool, bool)> = (0..4)
        .map(|_| {
            sim.tick();
            (sim.output("a").unwrap(), sim.output("b").unwrap())
        })
        .collect();
    assert_eq!(
        values,
        [(true, false), (false, true), (true, false), (false, true)]
    );
}

fn check_gate(stdlib: &Stdlib, module: ModuleId, expected: fn(bool, bool) -> bool) {
    let flat = flatten(module, &stdlib.library).unwrap();
    let mut sim = FlatSimulator::new(&flat);
    for (a, b) in [(false, false), (false, true), (true, false), (true, true)] {
        assert!(sim.set_input("a", a));
        assert!(sim.set_input("b", b));
        sim.tick();
        assert_eq!(sim.output("y"), Some(expected(a, b)), "a={a} b={b}");
    }
}

#[test]
fn elementary_stdlib_gates_match_truth_tables() {
    let stdlib = Stdlib::build();
    let flat = flatten(stdlib.not_gate, &stdlib.library).unwrap();
    let mut sim = FlatSimulator::new(&flat);
    for a in [false, true] {
        assert!(sim.set_input("a", a));
        sim.tick();
        assert_eq!(sim.output("y"), Some(!a));
    }
    check_gate(&stdlib, stdlib.and_gate, |a, b| a && b);
    check_gate(&stdlib, stdlib.or_gate, |a, b| a || b);
    check_gate(&stdlib, stdlib.nor_gate, |a, b| !(a || b));
    check_gate(&stdlib, stdlib.xor_gate, |a, b| a ^ b);
    check_gate(&stdlib, stdlib.xnor_gate, |a, b| !(a ^ b));
}

#[test]
fn mux_and_adders_match_reference_arithmetic() {
    let stdlib = Stdlib::build();

    let flat = flatten(stdlib.mux, &stdlib.library).unwrap();
    let mut sim = FlatSimulator::new(&flat);
    for sel in [false, true] {
        for a in [false, true] {
            for b in [false, true] {
                assert!(sim.set_input("sel", sel));
                assert!(sim.set_input("a", a));
                assert!(sim.set_input("b", b));
                sim.tick();
                assert_eq!(sim.output("y"), Some(if sel { b } else { a }));
            }
        }
    }

    let flat = flatten(stdlib.full_adder, &stdlib.library).unwrap();
    let mut sim = FlatSimulator::new(&flat);
    for bits in 0..8_u8 {
        let (a, b, cin) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
        assert!(sim.set_input("a", a));
        assert!(sim.set_input("b", b));
        assert!(sim.set_input("cin", cin));
        sim.tick();
        let total = u8::from(a) + u8::from(b) + u8::from(cin);
        assert_eq!(sim.output("sum"), Some(total & 1 != 0));
        assert_eq!(sim.output("cout"), Some(total >= 2));
    }

    let flat = flatten(stdlib.adder24, &stdlib.library).unwrap();
    let mut sim = FlatSimulator::new(&flat);
    for &(a, b, cin) in &[
        (0_u32, 0_u32, false),
        (1, 1, false),
        (0xFF_FFFF, 1, false),
        (0x123456, 0x654321, true),
        (0xFF_FFFF, 0xFF_FFFF, true),
    ] {
        for bit in 0..24 {
            assert!(sim.set_input(&format!("a.{bit}"), (a >> bit) & 1 != 0));
            assert!(sim.set_input(&format!("b.{bit}"), (b >> bit) & 1 != 0));
        }
        assert!(sim.set_input("cin", cin));
        sim.tick();
        let mut actual = 0_u64;
        for bit in 0..24 {
            if sim.output(&format!("sum.{bit}")) == Some(true) {
                actual |= 1 << bit;
            }
        }
        if sim.output("cout") == Some(true) {
            actual |= 1 << 24;
        }
        assert_eq!(actual, u64::from(a) + u64::from(b) + u64::from(cin));
    }
}

#[test]
fn multiplier_and_lfsr_match_software_models() {
    let stdlib = Stdlib::build();
    let flat = flatten(stdlib.mul16, &stdlib.library).unwrap();
    let mut sim = FlatSimulator::new(&flat);
    for &(a, b) in &[(0_u32, 0_u32), (1, 1), (0xFFFF, 0xFFFF), (12345, 54321)] {
        for bit in 0..16 {
            assert!(sim.set_input(&format!("a.{bit}"), (a >> bit) & 1 != 0));
            assert!(sim.set_input(&format!("b.{bit}"), (b >> bit) & 1 != 0));
        }
        sim.tick();
        let mut product = 0_u32;
        for bit in 0..32 {
            if sim.output(&format!("p.{bit}")) == Some(true) {
                product |= 1 << bit;
            }
        }
        assert_eq!(product, a * b);
    }

    let flat = flatten(stdlib.lfsr16, &stdlib.library).unwrap();
    let mut sim = FlatSimulator::new(&flat);
    let mut state = 1_u16;
    let mut period = None;
    for step in 1..=65_535_u64 {
        sim.tick();
        assert_eq!(sim.output("out"), Some((state >> 15) & 1 != 0));
        let feedback = ((state >> 15) ^ (state >> 13) ^ (state >> 12) ^ (state >> 10)) & 1;
        state = (state << 1) | feedback;
        if state == 1 && period.is_none() {
            period = Some(step);
        }
    }
    assert_eq!(period, Some(65_535));
}
