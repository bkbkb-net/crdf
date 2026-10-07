//! Building the glue a [`WordConvert`] describes.
//!
//! [`plan_connection`](crate::connect::plan_connection) *describes* a
//! value-preserving fixed-point conversion; [`build_word_convert`] here
//! *realises* it as NAND gates, using the generic [`Stdlib`] gate modules.
//! It is the arithmetic a fixed-point datapath otherwise writes by hand
//! (sign-extending shift, saturating clamp, negative clamp), derived from
//! the type pair instead.
//!
//! A conversion is: align the binary point by `shift` (a pure rewiring —
//! a shift is free in a circuit), then fit into the destination width. The
//! fit is free when the planner proved it cannot overflow; otherwise it
//! costs an overflow test plus a saturating/​clamping multiplexer per bit.

use crate::connect::WordConvert;
use crate::model::{Endpoint, ModuleBuilder};
use crate::stdlib::{Stdlib, gate_ports, port_id};

/// Builds the gates for `conv` inside module `m`, driven by the `input`
/// endpoints (`conv.src.width` of them, LSB-0), and returns the
/// `conv.dst.width` output endpoints (LSB-0). `zero` / `one` are constant
/// sources in `m` (e.g. `Endpoint::reg_q(m.constant(false|true))`).
///
/// `stdlib` supplies the generic gate modules (`xor`, `or`, `mux`); the
/// caller's asset library must contain them (as every stdlib-based build
/// already does).
pub fn build_word_convert(
    m: &mut ModuleBuilder,
    stdlib: &Stdlib,
    conv: &WordConvert,
    input: &[Endpoint],
    zero: Endpoint,
    one: Endpoint,
) -> Vec<Endpoint> {
    assert_eq!(
        input.len(),
        usize::from(conv.src.width),
        "input width must equal conv.src.width"
    );
    let sw = i32::from(conv.src.width);
    let dw = i32::from(conv.dst.width);
    let shift = conv.shift;

    // The true sign of the source value (0 for unsigned sources).
    let vsign = if conv.src.signed {
        input[(sw - 1) as usize]
    } else {
        zero
    };

    // The point-aligned ("ideal", pre-fit) bit at output position `p`:
    // input bit `p - shift`, with LSB zero-fill below and sign/zero
    // extension above the source word.
    let ideal = |p: i32| -> Endpoint {
        let j = p - shift;
        if j < 0 {
            zero
        } else if j < sw {
            input[j as usize]
        } else {
            vsign
        }
    };

    // Fast path: the planner proved the value fits — pure rewiring.
    if !conv.saturate && !conv.clamp_negative {
        return (0..dw).map(ideal).collect();
    }

    if conv.dst.signed {
        // Signed saturating fit. Overflow iff any bit from the sign
        // position up disagrees with the true sign (not a clean sign
        // extension); then clamp to +max (0x7…F) or −min (0x8…0).
        let guard_top = (sw - 1 + shift).max(dw - 1);
        let mut overflow: Option<Endpoint> = None;
        for p in (dw - 1)..=guard_top {
            let diff = xor2(m, stdlib, ideal(p), vsign);
            overflow = Some(match overflow {
                None => diff,
                Some(acc) => or2(m, stdlib, acc, diff),
            });
        }
        let overflow = overflow.expect("guard range is non-empty");
        let not_vsign = not1(m, vsign);
        (0..dw)
            .map(|p| {
                let sat = if p < dw - 1 { not_vsign } else { vsign };
                pick(m, stdlib, overflow, ideal(p), sat) // overflow ? sat : ideal
            })
            .collect()
    } else {
        // Unsigned fit: negatives clamp to 0, positive overflow to all-ones.
        let is_negative = vsign;
        let guard_top = (sw - 1 + shift).max(dw - 1);
        let mut overflow_high: Option<Endpoint> = None;
        for p in dw..=guard_top {
            let bit = ideal(p);
            overflow_high = Some(match overflow_high {
                None => bit,
                Some(acc) => or2(m, stdlib, acc, bit),
            });
        }
        let overflow_high = overflow_high.unwrap_or(zero);
        (0..dw)
            .map(|p| {
                let hi = pick(m, stdlib, overflow_high, ideal(p), one); // overflow ? 1 : ideal
                pick(m, stdlib, is_negative, hi, zero) // negative ? 0 : hi
            })
            .collect()
    }
}

fn xor2(m: &mut ModuleBuilder, stdlib: &Stdlib, a: Endpoint, b: Endpoint) -> Endpoint {
    let p = gate_ports(&stdlib.library, stdlib.xor_gate);
    let inst = m.instance(stdlib.xor_gate);
    m.wire(a, Endpoint::inner(inst, p.a));
    m.wire(b, Endpoint::inner(inst, p.b));
    Endpoint::inner(inst, p.y)
}

fn or2(m: &mut ModuleBuilder, stdlib: &Stdlib, a: Endpoint, b: Endpoint) -> Endpoint {
    let p = gate_ports(&stdlib.library, stdlib.or_gate);
    let inst = m.instance(stdlib.or_gate);
    m.wire(a, Endpoint::inner(inst, p.a));
    m.wire(b, Endpoint::inner(inst, p.b));
    Endpoint::inner(inst, p.y)
}

fn not1(m: &mut ModuleBuilder, a: Endpoint) -> Endpoint {
    Endpoint::nand_y(m.not_gate(a))
}

/// `sel ? when1 : when0` via `std.mux` (`y = sel ? b : a`).
fn pick(
    m: &mut ModuleBuilder,
    stdlib: &Stdlib,
    sel: Endpoint,
    when0: Endpoint,
    when1: Endpoint,
) -> Endpoint {
    let sel_p = port_id(&stdlib.library, stdlib.mux, "sel");
    let a_p = port_id(&stdlib.library, stdlib.mux, "a");
    let b_p = port_id(&stdlib.library, stdlib.mux, "b");
    let y_p = port_id(&stdlib.library, stdlib.mux, "y");
    let inst = m.instance(stdlib.mux);
    m.wire(sel, Endpoint::inner(inst, sel_p));
    m.wire(when0, Endpoint::inner(inst, a_p));
    m.wire(when1, Endpoint::inner(inst, b_p));
    Endpoint::inner(inst, y_p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::flatten;
    use crate::connect::{WordFit, plan_connection};
    use crate::interp::FlatSimulator;
    use crate::model::{Endpoint, ModuleBuilder};
    use crate::types::{Type, Word};

    /// The software reference: interpret `raw` as a `src` value, align the
    /// binary point by `conv.shift` (arithmetic), then clamp into `dst`.
    fn reference(conv: &WordConvert, raw: u64) -> u64 {
        let sw = conv.src.width;
        let dw = conv.dst.width;
        let mut v = (raw & mask(sw)) as i64;
        if conv.src.signed && (v >> (sw - 1)) & 1 == 1 {
            v -= 1i64 << sw;
        }
        let shifted = if conv.shift >= 0 {
            v << conv.shift
        } else {
            v >> (-conv.shift)
        };
        let clamped = if conv.dst.signed {
            let max = (1i64 << (dw - 1)) - 1;
            let min = -(1i64 << (dw - 1));
            shifted.clamp(min, max)
        } else {
            shifted.clamp(0, (1i64 << dw) - 1)
        };
        (clamped as u64) & mask(dw)
    }

    fn mask(width: u16) -> u64 {
        if width >= 64 {
            u64::MAX
        } else {
            (1u64 << width) - 1
        }
    }

    /// Builds the convert, then evaluates it on `raw_in` via the scalar
    /// reference simulator; returns the reconstructed dst raw word.
    fn eval_convert(conv: &WordConvert, raw_in: u64) -> u64 {
        let stdlib = Stdlib::build();
        let sw = conv.src.width;
        let dw = conv.dst.width;

        let mut m = ModuleBuilder::new("test.convert");
        let ins: Vec<_> = (0..sw).map(|i| m.input(format!("in.{i}"))).collect();
        let outs: Vec<_> = (0..dw).map(|i| m.output(format!("out.{i}"))).collect();
        let zero = Endpoint::reg_q(m.constant(false));
        let one = Endpoint::reg_q(m.constant(true));
        let in_e: Vec<Endpoint> = ins.iter().map(|&p| Endpoint::module(p)).collect();

        let out_e = build_word_convert(&mut m, &stdlib, conv, &in_e, zero, one);
        for (i, &e) in out_e.iter().enumerate() {
            m.wire(e, Endpoint::module(outs[i]));
        }
        let module = m.finish();
        let root = module.id;
        let mut lib = stdlib.library.clone();
        lib.insert(module);

        let flat = flatten(root, &lib).expect("flattens");
        let mut sim = FlatSimulator::new(&flat);
        for i in 0..sw {
            let bit = (raw_in >> i) & 1 == 1;
            assert!(sim.set_input(&format!("in.{i}"), bit));
        }
        // Combinational apart from the constant self-holds; a couple of
        // ticks settle everything.
        sim.tick();
        sim.tick();

        let mut raw_out = 0u64;
        for i in 0..dw {
            if sim.output(&format!("out.{i}")).expect("output exists") {
                raw_out |= 1u64 << i;
            }
        }
        raw_out
    }

    fn conv_of(src: Word, dst: Word) -> WordConvert {
        match plan_connection(&Type::word(src), &Type::word(dst))
            .unwrap()
            .fits[0]
        {
            WordFit::Convert(c) => c,
            WordFit::Direct => panic!("expected a conversion for {src:?} -> {dst:?}"),
        }
    }

    fn check(src: Word, dst: Word, raws: &[u64]) {
        let conv = conv_of(src, dst);
        for &raw in raws {
            let got = eval_convert(&conv, raw);
            let want = reference(&conv, raw);
            assert_eq!(
                got,
                want,
                "convert {} -> {} on raw {raw:#x}: got {got:#x}, want {want:#x}",
                src.q_format(),
                dst.q_format()
            );
        }
    }

    #[test]
    fn q1_15_to_q4_12_right_shift_matches_reference() {
        // Q1.15 -> Q4.12 (right shift 3, no saturation).
        let src = Word::signed_q(1, 15);
        let dst = Word::signed_q(4, 12);
        check(src, dst, &[0, 1, 8, 0x7FFF, 0x8000, 0xFFFF, 0x4000, 0xC000]);
    }

    #[test]
    fn q4_12_to_q1_15_left_shift_saturates() {
        // Q4.12 -> Q1.15 (left shift 3, saturating). Values above ±1
        // must clamp to the rails.
        let src = Word::signed_q(4, 12);
        let dst = Word::signed_q(1, 15);
        check(
            src,
            dst,
            &[
                0, 0x0001, 0x0FFF, 0x1000, 0x2000, 0x7FFF, 0x8000, 0xC000, 0xF000,
            ],
        );
    }

    #[test]
    fn signed_to_unsigned_clamps_negatives_to_zero() {
        // Q1.15 -> UQ1.15: negatives become 0, positives pass.
        let src = Word::signed_q(1, 15);
        let dst = Word::new(16, false, 15);
        check(src, dst, &[0, 1, 0x4000, 0x7FFF, 0x8000, 0xC000, 0xFFFF]);
    }

    #[test]
    fn widening_preserves_value() {
        // Q1.15 -> Q1.16 (wider, left shift 1, fits).
        let src = Word::signed_q(1, 15);
        let dst = Word::signed_q(1, 16);
        check(src, dst, &[0, 1, 0x7FFF, 0x8000, 0xFFFF]);
    }

    #[test]
    fn unsigned_narrowing_saturates() {
        // UQ8.0 -> UQ4.0: values above 15 clamp to 15.
        let src = Word::uint(8);
        let dst = Word::uint(4);
        check(src, dst, &[0, 5, 15, 16, 200, 255]);
    }

    #[test]
    fn exhaustive_small_conversions() {
        // Sweep every input of a few narrow conversions against the model.
        let cases = [
            (Word::sint(6), Word::signed_q(2, 2)), // left shift 2, may saturate
            (Word::signed_q(2, 4), Word::sint(4)), // right shift 4, truncate
            (Word::uint(5), Word::sint(4)),        // unsigned -> signed narrow
            (Word::sint(5), Word::uint(4)),        // signed -> unsigned clamp
        ];
        for (src, dst) in cases {
            let raws: Vec<u64> = (0..(1u64 << src.width)).collect();
            check(src, dst, &raws);
        }
    }
}
