//! Recognising register shift chains as delay banks.
//!
//! A run of registers whose `d` is the previous register's `q` is a
//! delay line. Evaluated literally it costs one word of state traffic
//! per register per tick, so it is linear in its own length; evaluated
//! as a ring buffer it costs one write and one read per tap. The
//! circuit is unchanged either way — this proves the shape from the
//! netlist and hands the evaluator a faster way to run the same thing,
//! exactly as the `fold` pass does for the full adder.
//!
//! Nothing here adds an axiom. A marker on a library module saying
//! "this is a delay" would be one in disguise, because a module that did
//! not have those semantics would still be given them; the equivalence
//! has to be read off the structure.
//!
//! What it deliberately does not solve: a *moving* tap is combinational
//! selection, and stays a mux.
//!
//! This is public because the answer is useful on its own: an editor can
//! ask how much of a patch is delay line, and therefore whether it will
//! lower well, before compiling it. The compiler consumes the same
//! answer — recognition has to run before slots are assigned, so it
//! cannot be folded into [`crate::compile::CompiledCircuit::from_flat`]
//! as a private step.

use crate::compile::{FlatCircuit, NetId};
use std::collections::BTreeMap;

/// Runs shorter than this stay ordinary registers.
///
/// Two reasons, and the second is temporary. Short chains are everywhere
/// — an LFSR is a shift register, and so is anything that stages a value
/// for a few ticks — and for those the ring's bookkeeping buys little.
/// More pressingly, while the JIT cannot evaluate a bank it refuses the
/// whole circuit, so lowering a chain that was never the bottleneck
/// trades a native-code circuit for an interpreted one and comes out
/// slower overall.
///
/// The threshold is therefore set well above the incidental shift
/// registers a circuit typically has (an LFSR's sixteen, a few ticks of
/// staging), so that only lines long enough to be *delays* are claimed. **Lower it once the JIT understands banks**: at that
/// point the trade-off disappears and shorter chains become free wins.
pub const MIN_CHAIN: usize = 128;

/// A recognised shift chain, in nets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatBank {
    /// Ticks of delay, and ring entries.
    pub len: u32,
    /// Shared reset value of every register in the run.
    pub init: bool,
    /// The signal entering the line.
    pub ingress: NetId,
    /// Nets that must hold a value read back out, by how many ticks of
    /// delay they sit at. Distances run `1..=len`; the ingress is not a
    /// tap, and `len` is the chain's final output.
    pub taps: Vec<(u32, NetId)>,
}

/// What recognition found.
pub struct Recognised {
    pub banks: Vec<FlatBank>,
    /// Register indices now owned by a bank, in `flat.regs` order.
    pub claimed: Vec<bool>,
}

impl Recognised {
    pub fn none(reg_count: usize) -> Self {
        Self {
            banks: Vec::new(),
            claimed: vec![false; reg_count],
        }
    }
}

/// Finds the shift chains in a flattened circuit.
///
/// Runs before slots are assigned, because the evaluator requires the
/// registers that survive to occupy exactly the first slots — eliding
/// registers after the fact would punch holes in that range.
pub fn recognise(flat: &FlatCircuit) -> Recognised {
    let reg_count = flat.regs.len();
    if reg_count < MIN_CHAIN {
        return Recognised::none(reg_count);
    }

    // net -> the register that drives it, for register outputs only.
    let mut driver: BTreeMap<NetId, usize> = BTreeMap::new();
    for (index, reg) in flat.regs.iter().enumerate() {
        driver.insert(reg.q, index);
    }

    // Link edges: register `k` reads register `pred[k]`'s output, and
    // nothing else does.
    let mut pred: Vec<Option<usize>> = vec![None; reg_count];
    let mut succ: Vec<Option<usize>> = vec![None; reg_count];
    let mut successors = vec![0u32; reg_count];
    for (index, reg) in flat.regs.iter().enumerate() {
        let Some(&from) = driver.get(&reg.d) else {
            continue;
        };
        if from == index {
            // A register feeding itself is a one-entry ring, not a chain.
            continue;
        }
        pred[index] = Some(from);
        successors[from] += 1;
        succ[from] = Some(index);
    }
    // Two readers means the line would have to branch; leave the whole
    // junction alone rather than guess which side is the chain. Counted
    // rather than swept, so this stays linear in the register count —
    // the whole point is to admit lines of hundreds of thousands.
    for index in 0..reg_count {
        if successors[index] > 1 {
            succ[index] = None;
        }
    }
    for link in pred.iter_mut() {
        if let Some(from) = *link
            && successors[from] > 1
        {
            *link = None;
        }
    }

    // Who reads a net combinationally or at the boundary, and how many
    // registers read it. A link's output is read by exactly one register
    // — its successor — and by nothing else; anything more makes it a
    // tap the bank has to publish.
    let readers = external_readers(flat);
    let mut reg_readers: BTreeMap<NetId, u32> = BTreeMap::new();
    for reg in &flat.regs {
        *reg_readers.entry(reg.d).or_insert(0) += 1;
    }

    // A run's head is a register with no usable predecessor. "Usable"
    // has to include the reset value: where `init` changes, the chain
    // is cut and the register after the cut heads a *new* run even
    // though it does have a predecessor. Deciding this statically keeps
    // runs disjoint, so nothing has to be walked backwards.
    let is_head = |k: usize| match pred[k] {
        None => true,
        Some(from) => flat.regs[from].init != flat.regs[k].init,
    };

    let mut claimed = vec![false; reg_count];
    let mut banks = Vec::new();
    for start in 0..reg_count {
        if claimed[start] || !is_head(start) {
            continue;
        }
        let Some(run) = walk(start, &succ, flat) else {
            continue;
        };
        if run.len() < MIN_CHAIN {
            continue;
        }
        let init = flat.regs[run[0]].init;
        let mut taps = Vec::new();
        for (offset, &reg) in run.iter().enumerate() {
            let net = flat.regs[reg].q;
            let distance = offset as u32 + 1;
            // The last register's output is the line's own output and is
            // always published, whether or not anything reads it today.
            let only_its_successor = reg_readers.get(&net).copied().unwrap_or(0) <= 1;
            let internal_only =
                distance < run.len() as u32 && !readers.contains(&net) && only_its_successor;
            if !internal_only {
                taps.push((distance, net));
            }
        }
        for &reg in &run {
            claimed[reg] = true;
        }
        banks.push(FlatBank {
            len: run.len() as u32,
            init,
            ingress: flat.regs[run[0]].d,
            taps,
        });
    }

    Recognised { banks, claimed }
}

/// Walks forward from a run's head, stopping where the chain stops.
///
/// A ring of registers is a legal circuit — only *combinational* cycles
/// are rejected, and a rotating pattern generator is exactly a ring — so
/// a uniform ring has no head under `is_head` and is never walked at
/// all. The length bound here is the belt to that braces: it costs one
/// comparison per step and cannot be defeated by a shape nobody thought
/// of, whereas a visited set would cost a vector per run.
fn walk(start: usize, succ: &[Option<usize>], flat: &FlatCircuit) -> Option<Vec<usize>> {
    let init = flat.regs[start].init;
    let mut run = vec![start];
    let mut at = start;
    while let Some(next) = succ[at] {
        if flat.regs[next].init != init {
            // A different reset value cannot share one ring.
            break;
        }
        if run.len() > succ.len() {
            return None;
        }
        run.push(next);
        at = next;
    }
    Some(run)
}

/// Every net read by something that is not a register's `d`.
///
/// A register's `d` is handled by the chain walk; everything here is an
/// outside reader and forces the net to stay live as a tap.
fn external_readers(flat: &FlatCircuit) -> std::collections::BTreeSet<NetId> {
    let mut readers = std::collections::BTreeSet::new();
    for gate in &flat.gates {
        readers.insert(gate.a);
        readers.insert(gate.b);
    }
    for (_, net) in &flat.outputs {
        readers.insert(*net);
    }
    // A top-level input bound onto a net makes it externally driven, so
    // it cannot be a chain's internal link either.
    for (_, net) in &flat.inputs {
        readers.insert(*net);
    }
    readers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::flatten;
    use crate::model::{Endpoint, ModuleBuilder, ModuleLibrary};

    /// `in -> reg * stages -> out`, optionally with an extra reader on
    /// the net `tap_after` registers along.
    fn chain(stages: usize, tap_after: Option<usize>) -> FlatCircuit {
        let mut m = ModuleBuilder::new("chain");
        let input = m.input("in");
        let out = m.output("out");
        let mut at = Endpoint::module(input);
        let mut tapped = None;
        for index in 0..stages {
            let reg = m.reg(false);
            m.wire(at, Endpoint::reg_d(reg));
            at = Endpoint::reg_q(reg);
            if tap_after == Some(index + 1) {
                tapped = Some(at);
            }
        }
        m.wire(at, Endpoint::module(out));
        if let Some(tapped) = tapped {
            // A second output reading the middle of the line.
            let mid = m.output("mid");
            m.wire(tapped, Endpoint::module(mid));
        }
        let root = m.id();
        let mut library = ModuleLibrary::new();
        library.insert(m.finish());
        flatten(root, &library).expect("the chain flattens")
    }

    #[test]
    fn a_plain_chain_becomes_one_bank_with_a_single_tap() {
        let flat = chain(MIN_CHAIN, None);
        let found = recognise(&flat);
        assert_eq!(found.banks.len(), 1);
        let bank = &found.banks[0];
        assert_eq!(bank.len, MIN_CHAIN as u32);
        assert_eq!(
            bank.taps.len(),
            1,
            "only the line's own output needs publishing"
        );
        assert_eq!(
            bank.taps[0].0, MIN_CHAIN as u32,
            "and it sits at the far end"
        );
        assert!(
            found.claimed.iter().all(|claimed| *claimed),
            "every register of the run belongs to the bank"
        );
    }

    #[test]
    fn a_read_partway_along_becomes_a_second_tap() {
        let flat = chain(MIN_CHAIN, Some(20));
        let found = recognise(&flat);
        assert_eq!(found.banks.len(), 1);
        let bank = &found.banks[0];
        let mut distances: Vec<u32> = bank.taps.iter().map(|(d, _)| *d).collect();
        distances.sort_unstable();
        assert_eq!(
            distances,
            vec![20, MIN_CHAIN as u32],
            "the middle read stays live"
        );
    }

    #[test]
    fn a_run_shorter_than_the_threshold_is_left_alone() {
        let found = recognise(&chain(MIN_CHAIN - 1, None));
        assert!(found.banks.is_empty());
        assert!(found.claimed.iter().all(|claimed| !*claimed));
    }

    /// A ring of registers is a legal circuit — only combinational
    /// cycles are rejected — so recognition must notice and decline
    /// rather than walk round it for ever.
    #[test]
    fn a_register_ring_is_declined_rather_than_walked_for_ever() {
        let mut m = ModuleBuilder::new("ring");
        let out = m.output("out");
        let regs: Vec<_> = (0..32).map(|_| m.reg(false)).collect();
        for pair in regs.windows(2) {
            m.wire(Endpoint::reg_q(pair[0]), Endpoint::reg_d(pair[1]));
        }
        // Close the loop: the last register drives the first.
        m.wire(
            Endpoint::reg_q(*regs.last().unwrap()),
            Endpoint::reg_d(regs[0]),
        );
        m.wire(
            Endpoint::reg_q(*regs.last().unwrap()),
            Endpoint::module(out),
        );
        let root = m.id();
        let mut library = ModuleLibrary::new();
        library.insert(m.finish());
        let flat = flatten(root, &library).expect("a register ring is a legal circuit");

        let found = recognise(&flat);
        assert!(
            found.banks.is_empty(),
            "a ring has no head, so there is no line to lower"
        );
    }

    #[test]
    fn a_chain_whose_output_fans_out_to_two_registers_stops_there() {
        let mut m = ModuleBuilder::new("fork");
        let input = m.input("in");
        let mut at = Endpoint::module(input);
        for _ in 0..MIN_CHAIN {
            let reg = m.reg(false);
            m.wire(at, Endpoint::reg_d(reg));
            at = Endpoint::reg_q(reg);
        }
        // Two registers read the same output: the line would branch.
        for name in ["a", "b"] {
            let reg = m.reg(false);
            m.wire(at, Endpoint::reg_d(reg));
            let out = m.output(name);
            m.wire(Endpoint::reg_q(reg), Endpoint::module(out));
        }
        let root = m.id();
        let mut library = ModuleLibrary::new();
        library.insert(m.finish());
        let flat = flatten(root, &library).expect("the fork flattens");

        let found = recognise(&flat);
        assert_eq!(found.banks.len(), 1, "the trunk still lowers");
        let bank = &found.banks[0];
        assert_eq!(bank.len, MIN_CHAIN as u32, "and stops at the junction");
        assert_eq!(bank.taps.last().map(|(d, _)| *d), Some(MIN_CHAIN as u32));
    }

    #[test]
    fn a_different_reset_value_ends_the_run() {
        let mut m = ModuleBuilder::new("mixed");
        let input = m.input("in");
        let out = m.output("out");
        let mut at = Endpoint::module(input);
        for index in 0..(MIN_CHAIN * 2) {
            let reg = m.reg(index >= MIN_CHAIN);
            m.wire(at, Endpoint::reg_d(reg));
            at = Endpoint::reg_q(reg);
        }
        m.wire(at, Endpoint::module(out));
        let root = m.id();
        let mut library = ModuleLibrary::new();
        library.insert(m.finish());
        let flat = flatten(root, &library).expect("the chain flattens");

        let found = recognise(&flat);
        // Two runs of twenty, each with one reset value; a ring holds one.
        assert_eq!(found.banks.len(), 2);
        assert!(found.banks.iter().all(|bank| bank.len == MIN_CHAIN as u32));
        assert_ne!(found.banks[0].init, found.banks[1].init);
    }
}
