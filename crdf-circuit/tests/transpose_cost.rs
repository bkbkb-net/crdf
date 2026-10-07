//! What it costs to change representation — measured, not estimated.

use crdf_circuit::transpose::{lanes_to_planes, planes_to_lanes};
use std::hint::black_box;
use std::time::Instant;

/// The conversion is exact and reversible, which is the part that has
/// to be true before the timing is worth anything.
#[test]
fn a_round_trip_returns_the_same_bits() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for _ in 0..200 {
        let mut planes = [0_u64; 32];
        for p in planes.iter_mut() {
            *p = next();
        }
        let lanes = planes_to_lanes(black_box(&planes));
        assert_eq!(lanes_to_planes(&lanes), planes, "round trip");

        // And it is the transpose it claims to be, checked the slow way
        // on a few positions rather than trusting the block exchange.
        for (bit, plane) in planes.iter().enumerate() {
            for lane in [0_usize, 1, 31, 32, 63] {
                assert_eq!(
                    (plane >> lane) & 1 == 1,
                    (lanes[lane] >> bit) & 1 == 1,
                    "bit {bit} of lane {lane}"
                );
            }
        }
    }
}

/// **The number the substitution decision rests on.**
///
/// Replacing a sub-circuit with a native instruction only pays if the
/// gates it removes cost more than the two transposes it adds. Before
/// this test that trade-off was being argued from an estimate with a
/// five-fold spread, which is wide enough for the answer to flip inside
/// it.
///
/// This is a timing test, so it asserts only a ceiling loose enough to
/// survive a loaded machine, and prints the figure. The figure is what
/// it is for; the assertion is only there to catch a change that makes
/// the conversion pathological.
#[test]
fn a_transpose_pair_costs_well_under_a_microsecond() {
    let mut planes = [0_u64; 32];
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    for p in planes.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *p = state;
    }

    // Warm up, then time enough pairs that the clock's resolution is
    // not part of the answer.
    let mut sink = 0_u64;
    for _ in 0..1_000 {
        let lanes = planes_to_lanes(black_box(&planes));
        sink ^= u64::from(lanes[0]);
    }

    const ROUNDS: u32 = 200_000;
    let started = Instant::now();
    for _ in 0..ROUNDS {
        let lanes = planes_to_lanes(black_box(&planes));
        let back = lanes_to_planes(black_box(&lanes));
        sink ^= black_box(back)[0];
        planes[0] = planes[0].rotate_left(1);
    }
    let elapsed = started.elapsed();
    // `black_box` above is what keeps the work; this only stops the
    // accumulator itself being dead. An assertion here was worse than
    // useless -- it fired the first time the sum happened to land on
    // the sentinel, which says nothing about the transpose.
    black_box(sink);

    let nanos_per_pair = elapsed.as_nanos() as f64 / f64::from(ROUNDS);
    println!("transpose pair (64 lanes x 32 bits, both ways): {nanos_per_pair:.0} ns");

    // Only an optimised build measures the algorithm; an unoptimised one
    // measures the absence of the optimiser, and runs about ten times
    // slower here. Printing it is still useful, asserting on it is not.
    if cfg!(debug_assertions) {
        println!("  (unoptimised build -- figure is not the one to quote)");
        return;
    }

    assert!(
        nanos_per_pair < 1_000.0,
        "a transpose pair took {nanos_per_pair:.0} ns. The block exchange \
         is a few hundred operations, so this is not a slow machine -- it \
         is a change that made the conversion pathological, and the \
         substitution arithmetic that rests on this number is now wrong"
    );
}
