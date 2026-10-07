//! The binary32 multiply: what it equals, what it costs, and what it
//! does at the edges.
//!
//! It is generic arithmetic, like the fixed-point multiply.

use crdf_circuit::compile::flatten;
use crdf_circuit::interp::FlatSimulator;
use crdf_circuit::stdlib::Stdlib;

/// **The float circuit equals the hardware, bit for bit.**
///
/// Not "agrees to a unit in the last place" — *equals*. That is the
/// whole point of the circuit and the only property that would let a
/// JIT put a native `fmul` in its place: a substitution is only sound
/// for a netlist the instruction is equal to, and one wrong bit in the
/// last place is not equal.
///
/// The first version of this circuit truncated, and this test accepted
/// a one-ULP gap and only twelve hand-picked cases, so the gap went
/// unnoticed until a review asked whether the substitution was actually
/// sound. It was not. The circuit now rounds to nearest with ties to
/// even, and the test below allows no tolerance at all.
///
/// The domain is what the doc comment claims and no more: normal
/// values, no NaN, no infinity, no subnormals. Randomised exponents are
/// kept well inside the range so a product cannot leave it.
#[test]
fn fmul32_equals_the_hardware_bit_for_bit() {
    let stdlib = Stdlib::build();
    let flat = flatten(stdlib.fmul32, &stdlib.library).expect("flattens");
    let mut sim = FlatSimulator::new(&flat);

    let mul = |sim: &mut FlatSimulator, x: f32, y: f32| -> u32 {
        let (xb, yb) = (x.to_bits(), y.to_bits());
        for bit in 0..32 {
            assert!(sim.set_input(&format!("a.{bit}"), (xb >> bit) & 1 == 1));
            assert!(sim.set_input(&format!("b.{bit}"), (yb >> bit) & 1 == 1));
        }
        sim.tick();
        let mut got = 0_u32;
        for bit in 0..32 {
            if sim.output(&format!("out.{bit}")).expect("an output bit") {
                got |= 1 << bit;
            }
        }
        got
    };

    let check = |sim: &mut FlatSimulator, x: f32, y: f32| {
        let got = mul(sim, x, y);
        let want = x * y;
        assert_eq!(
            got,
            want.to_bits(),
            "{x} x {y}: circuit {} ({got:#010x}) vs hardware {want} ({:#010x})",
            f32::from_bits(got),
            want.to_bits()
        );
    };

    // Named cases first, including the ones that exercise rounding: a
    // tie that must go down to even, a tie that must go up to even, and
    // a mantissa that rounds from all-ones up through a power of two
    // and so carries into the exponent.
    for (x, y) in [
        (1.0_f32, 1.0_f32),
        (2.0, 3.0),
        (0.5, 0.5),
        (-1.5, 2.0),
        (1.5, -4.0),
        (-2.5, -2.5),
        (0.0, 5.0),
        (7.0, 0.0),
        (440.0, 2.0),
        (1.0 / 3.0, 3.0),
        (1.25, 1.25),
        (100.0, 0.01),
        // 2^-24 below two, squared: the product needs a round that
        // carries all the way out of the mantissa.
        (2.0 - f32::EPSILON, 2.0 - f32::EPSILON),
        (1.0 + f32::EPSILON, 1.0 + f32::EPSILON),
        (f32::from_bits(0x3f800001), f32::from_bits(0x3f800003)),
    ] {
        check(&mut sim, x, y);
    }

    // Then a few thousand random pairs. Exponents are held near zero so
    // the product stays normal; mantissas are fully random, which is
    // where rounding actually gets exercised.
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..4000 {
        let r = next();
        let mantissa_a = (r as u32) & 0x007f_ffff;
        let mantissa_b = ((r >> 32) as u32) & 0x007f_ffff;
        let exp_a = 127 + ((r >> 24) as i32 & 0x1f) - 16;
        let exp_b = 127 + ((r >> 56) as i32 & 0x1f) - 16;
        let sign_a = ((r >> 23) & 1) as u32;
        let sign_b = ((r >> 55) & 1) as u32;
        let x = f32::from_bits((sign_a << 31) | ((exp_a as u32) << 23) | mantissa_a);
        let y = f32::from_bits((sign_b << 31) | ((exp_b as u32) << 23) | mantissa_b);
        check(&mut sim, x, y);
    }
}

/// **What a float costs, measured.**
///
/// This is why the number was worth building the circuit for. Floating
/// point in gates is not cheaper than fixed point — it is several times
/// dearer, because it *contains* a wider integer multiply and then adds
/// exponent arithmetic, normalisation and rounding on top.
///
/// Rounding to nearest is 291 of those gates — a guard bit, a sticky
/// reduction over the discarded tail, the tie-to-even decision and a
/// 23-bit incrementer. Four per cent for the property that makes the
/// substitution sound at all; truncating to save it would save nothing,
/// because a circuit a native instruction cannot replace is a circuit
/// whose gates are all you ever get.
/// So `std.fmul32` is not a way to make patches cheaper. Its only route
/// to being worth its gates is a JIT that substitutes a native
/// instruction for the whole region.
#[test]
fn fmul32_costs_what_it_costs() {
    let stdlib = Stdlib::build();
    let float = flatten(stdlib.fmul32, &stdlib.library).expect("flattens");
    let fixed = flatten(stdlib.mul16, &stdlib.library).expect("flattens");

    // Written down rather than compared to themselves, so the next change
    // to either has something to fail against.
    assert_eq!(fixed.gate_count(), 2_672, "the 16x16 fixed-point multiply");
    assert_eq!(
        float.gate_count(),
        7_211,
        "the binary32 multiply: 2.7x the fixed-point one it would \
         replace, which is the finding this circuit was built to produce"
    );
}

/// **Overflow saturates, and underflow flushes.**
///
/// The doc comment has always said the circuit saturates at the largest
/// finite value rather than producing an infinity. It did not. Every
/// exponent bit went high on overflow, and all-ones is exactly the
/// encoding of infinity and NaN — `3e38 * 2` came out NaN from a circuit
/// that promised the opposite. A review caught it by reading; the
/// exactness test above could not, because it deliberately keeps its
/// inputs inside the normal range.
///
/// This is not IEEE behaviour and does not claim to be. It is the
/// behaviour the doc comment states, now actually implemented, which is
/// the property a lowering has to match.
#[test]
fn fmul32_saturates_rather_than_reaching_infinity() {
    let stdlib = Stdlib::build();
    let flat = flatten(stdlib.fmul32, &stdlib.library).expect("flattens");
    let mut sim = FlatSimulator::new(&flat);

    let mul = |sim: &mut FlatSimulator, x: f32, y: f32| -> u32 {
        let (xb, yb) = (x.to_bits(), y.to_bits());
        for bit in 0..32 {
            assert!(sim.set_input(&format!("a.{bit}"), (xb >> bit) & 1 == 1));
            assert!(sim.set_input(&format!("b.{bit}"), (yb >> bit) & 1 == 1));
        }
        sim.tick();
        let mut got = 0_u32;
        for bit in 0..32 {
            if sim.output(&format!("out.{bit}")).expect("an output bit") {
                got |= 1 << bit;
            }
        }
        got
    };

    for (x, y, sign) in [
        (3.0e38_f32, 2.0_f32, 0),
        (1.0e38, 1.0e38, 0),
        (-3.0e38, 2.0, 1),
        (3.0e38, -2.0, 1),
        // The ten-bit exponent is exactly 256: low eight bits are zero,
        // but this is overflow, not underflow.
        (f32::from_bits(0x7f00_0000), 4.0, 0),
    ] {
        let got = mul(&mut sim, x, y);
        let value = f32::from_bits(got);
        assert!(!value.is_nan(), "{x} x {y} gave NaN");
        assert!(!value.is_infinite(), "{x} x {y} gave an infinity");
        assert_eq!(
            got,
            f32::MAX.to_bits() | (sign << 31),
            "{x} x {y}: expected the largest finite value with the right sign"
        );
    }

    // And the other end still flushes to zero rather than going
    // subnormal. The sign survives, so a negative underflow is negative
    // zero, including when hardware would produce a subnormal.
    for (x, y, sign) in [
        (1.0e-30_f32, 1.0e-30_f32, 0_u32),
        (-1.0e-30, 1.0e-30, 1),
        (-1.0e-30, -1.0e-30, 0),
        (f32::MIN_POSITIVE, 0.75, 0),
        (-f32::MIN_POSITIVE, 0.75, 1),
    ] {
        assert_eq!(
            mul(&mut sim, x, y),
            sign << 31,
            "{x} x {y} should flush to zero, keeping its sign"
        );
    }

    // Exponent one is the first normal exponent and must survive.
    for (x, y) in [
        (f32::MIN_POSITIVE, 1.0),
        (f32::MIN_POSITIVE, 1.5),
        (-f32::MIN_POSITIVE, 1.5),
    ] {
        assert_eq!(mul(&mut sim, x, y), (x * y).to_bits());
    }
}
