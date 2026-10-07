//! Word-level folding: structural recognition of full-adder patterns.
//!
//! Cranelift (and any general-purpose compiler) performs value-level
//! optimisation only; recognising that 9 NAND gates form a full adder
//! is a logic-synthesis concern (the ABC / WordRev family), so it
//! happens here, before code generation, on the flattened netlist.
//!
//! The matcher recognises exactly the canonical 9-NAND full adder the
//! standard library emits (and any user circuit wired the same way):
//!
//! ```text
//! n1 = nand(a, b)      n4 = nand(n2, n3)   = a ^ b
//! n2 = nand(a, n1)     n5 = nand(n4, cin)
//! n3 = nand(b, n1)     n6 = nand(n4, n5)   n8 = nand(n6, n7) = sum
//!                      n7 = nand(cin, n5)  n9 = nand(n5, n1) = cout
//! ```
//!
//! Each match is replaced by one fused op evaluated with the
//! generate/propagate form (`p = a^b; sum = p^cin;
//! cout = (a&b)|(p&cin)`): 5 bitwise ops instead of 9 NANDs (18 SSA
//! ops in the JIT). Interior nets must have no other consumers and
//! must not feed registers or top-level outputs, so the fold is
//! behaviour-preserving by construction; the backend parity tests
//! verify it sample-exactly against the unfolded reference simulator.

use crate::compile::{FlatCircuit, FlatGate, NetId, RegionId};

/// One operation of the folded netlist, in evaluation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FoldedAnd {
    pub a: NetId,
    pub b: NetId,
    pub out: NetId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FoldedNot {
    pub a: NetId,
    pub out: NetId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlatOp {
    Nand(FlatGate),
    FullAdder(FoldedAdder),
    Xor(FoldedXor),
    /// `!(a & b)` immediately inverted: an AND, in two NANDs. The
    /// standard way to build one, and so extremely common.
    And(FoldedAnd),
    /// A NAND with both inputs on the same net: an inverter.
    Not(FoldedNot),
    /// An intrinsic region, scheduled as a single node. The index
    /// selects a [`FoldedRegion`] in the netlist's side table.
    ///
    /// It has to be one node rather than a marking on its gates: a
    /// backend that computes the whole region at once needs a point in
    /// the schedule where every one of its inputs is ready and none of
    /// its outputs is yet needed, and no annotation on individual gates
    /// says where that point is.
    Region(u32),
}

/// The canonical 4-NAND XOR (`n1..n4` of the adder diagram), folded to
/// one bitwise op. Matched after full adders so an adder front-end is
/// never split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FoldedXor {
    pub a: NetId,
    pub b: NetId,
    pub out: NetId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FoldedAdder {
    pub a: NetId,
    pub b: NetId,
    pub cin: NetId,
    pub sum: NetId,
    pub cout: NetId,
}

/// An intrinsic region taken whole: the nets on its boundary, and the
/// folded ops that compute it when no backend offers anything faster.
///
/// The fallback is not a second definition. It is the region's own
/// gates, folded exactly as any other gates would be, so a backend with
/// no lowering registered emits it and is right by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoldedRegion {
    pub region: RegionId,
    pub inputs: Vec<NetId>,
    pub outputs: Vec<NetId>,
    pub fallback: Vec<FlatOp>,
}

pub(crate) struct FoldedNetlist {
    pub ops: Vec<FlatOp>,
    pub regions: Vec<FoldedRegion>,
}

impl FlatOp {
    fn for_each_input(&self, regions: &[FoldedRegion], mut f: impl FnMut(NetId)) {
        match self {
            FlatOp::Nand(gate) => {
                f(gate.a);
                f(gate.b);
            }
            FlatOp::FullAdder(adder) => {
                f(adder.a);
                f(adder.b);
                f(adder.cin);
            }
            FlatOp::Xor(xor) => {
                f(xor.a);
                f(xor.b);
            }
            FlatOp::And(and) => {
                f(and.a);
                f(and.b);
            }
            FlatOp::Not(not) => f(not.a),
            FlatOp::Region(index) => {
                for net in &regions[*index as usize].inputs {
                    f(*net);
                }
            }
        }
    }

    fn for_each_output(&self, regions: &[FoldedRegion], mut f: impl FnMut(NetId)) {
        match self {
            FlatOp::Nand(gate) => f(gate.y),
            FlatOp::FullAdder(adder) => {
                f(adder.sum);
                f(adder.cout);
            }
            FlatOp::Xor(xor) => f(xor.out),
            FlatOp::And(and) => f(and.out),
            FlatOp::Not(not) => f(not.out),
            FlatOp::Region(index) => {
                for net in &regions[*index as usize].outputs {
                    f(*net);
                }
            }
        }
    }
}

/// Folds full adders, then XOR trees, and returns the ops re-sorted
/// topologically. Adders run first so an adder's XOR front-end is
/// never claimed by the weaker pattern.
///
/// Gates inside an intrinsic region are folded too — into that region's
/// fallback rather than the main list — and a pattern is never matched
/// across a region boundary. Folding an adder half inside a region and
/// half outside would leave one op that has to be both emitted and
/// skipped.
pub(crate) fn fold_netlist(flat: &FlatCircuit, banked: &[bool]) -> FoldedNetlist {
    let mut disabled = vec![false; flat.intrinsics().len()];
    // Each round disables at least one region, so this ends.
    for _ in 0..=disabled.len() {
        match fold_with_regions(flat, &disabled, banked) {
            Ok(netlist) => return netlist,
            Err(stuck) => {
                if stuck.is_empty() {
                    // A gate-level cycle, which flattening already
                    // rejects. Nothing left to disable, so give up on
                    // substitution entirely rather than loop.
                    disabled.iter_mut().for_each(|d| *d = true);
                } else {
                    for region in stuck {
                        disabled[region as usize] = true;
                    }
                }
            }
        }
    }
    fold_with_regions(flat, &vec![true; disabled.len()], banked)
        .expect("a gate-level fold always schedules")
}

/// Folds with the regions that are not `disabled` taken as single
/// nodes. On failure, returns the regions that could not be scheduled.
fn fold_with_regions(
    flat: &FlatCircuit,
    disabled: &[bool],
    banked: &[bool],
) -> Result<FoldedNetlist, Vec<RegionId>> {
    let atomic: Vec<bool> = atomic_regions(flat, banked)
        .into_iter()
        .zip(disabled)
        .map(|(ok, off)| ok && !*off)
        .collect();
    let region_of =
        |gate: &FlatGate| -> Option<RegionId> { gate.region.filter(|r| atomic[*r as usize]) };

    let matcher = Matcher::new(flat);
    let mut claimed = vec![false; flat.gates.len()];
    let mut adders: Vec<(FoldedAdder, Option<RegionId>)> = Vec::new();
    let mut xors: Vec<(FoldedXor, Option<RegionId>)> = Vec::new();

    // All gates of a match must sit in the same region, or in none.
    let same_region = |gates: &[usize]| -> Option<Option<RegionId>> {
        let first = region_of(&flat.gates()[gates[0]]);
        gates
            .iter()
            .all(|g| region_of(&flat.gates()[*g]) == first)
            .then_some(first)
    };

    for candidate in 0..flat.gates.len() {
        if claimed[candidate] {
            continue;
        }
        if let Some((adder, gates)) = matcher.match_full_adder(candidate, &claimed) {
            let Some(region) = same_region(&gates) else {
                continue;
            };
            for gate in gates {
                claimed[gate] = true;
            }
            adders.push((adder, region));
        }
    }
    for candidate in 0..flat.gates.len() {
        if claimed[candidate] {
            continue;
        }
        if let Some((xor, gates)) = matcher.match_xor(candidate, &claimed) {
            let Some(region) = same_region(&gates) else {
                continue;
            };
            for gate in gates {
                claimed[gate] = true;
            }
            xors.push((xor, region));
        }
    }

    // Then ANDs: a NAND immediately inverted. The standard way to build
    // one when NAND is all there is, and so the commonest shape in any
    // library -- a 16x16 multiply is 240 full adders and 256 of these.
    // Matched after adders so an adder's own `n1` is never taken.
    let mut ands: Vec<(FoldedAnd, Option<RegionId>)> = Vec::new();
    for candidate in 0..flat.gates.len() {
        if claimed[candidate] {
            continue;
        }
        if let Some((and, gates)) = matcher.match_and(candidate, &claimed) {
            let Some(region) = same_region(&gates) else {
                continue;
            };
            for gate in gates {
                claimed[gate] = true;
            }
            ands.push((and, region));
        }
    }
    // And whatever inverters are left over on their own.
    let mut nots: Vec<(FoldedNot, Option<RegionId>)> = Vec::new();
    for (candidate, gate) in flat.gates().iter().enumerate() {
        if claimed[candidate] || gate.a != gate.b {
            continue;
        }
        claimed[candidate] = true;
        nots.push((
            FoldedNot {
                a: gate.a,
                out: gate.y,
            },
            region_of(gate),
        ));
    }

    // Only the regions that can be taken whole get an entry. A
    // declaration that cannot -- one with state, or a pass-through
    // output -- is not offered for substitution at all, so it must not
    // appear in the list a backend looks through. Reporting one that
    // will never be emitted would let a lowering be registered, matched
    // and silently never used.
    let mut compact_of: Vec<Option<u32>> = vec![None; atomic.len()];
    let mut regions: Vec<FoldedRegion> = Vec::new();
    for (index, is_atomic) in atomic.iter().enumerate() {
        if *is_atomic {
            let decl = &flat.intrinsics()[index];
            compact_of[index] = Some(regions.len() as u32);
            regions.push(FoldedRegion {
                region: index as RegionId,
                inputs: decl.inputs.clone(),
                outputs: decl.outputs.clone(),
                fallback: Vec::new(),
            });
        }
    }

    let mut ops: Vec<FlatOp> = Vec::with_capacity(flat.gates.len());
    let mut push = |op: FlatOp, region: Option<RegionId>, ops: &mut Vec<FlatOp>| match region {
        Some(r) => regions[compact_of[r as usize].expect("atomic") as usize]
            .fallback
            .push(op),
        None => ops.push(op),
    };
    for (index, gate) in flat.gates.iter().enumerate() {
        if !claimed[index] {
            push(FlatOp::Nand(*gate), region_of(gate), &mut ops);
        }
    }
    for (adder, region) in adders {
        push(FlatOp::FullAdder(adder), region, &mut ops);
    }
    for (xor, region) in xors {
        push(FlatOp::Xor(xor), region, &mut ops);
    }
    for (and, region) in ands {
        push(FlatOp::And(and), region, &mut ops);
    }
    for (not, region) in nots {
        push(FlatOp::Not(not), region, &mut ops);
    }
    for index in 0..regions.len() {
        ops.push(FlatOp::Region(index as u32));
    }

    // Each region's fallback is sorted on its own; nothing outside it
    // can sit between two of its ops, which is what makes the region a
    // single node. A fallback holds only that region's gates, so it is
    // gate-level and always schedules.
    for region in regions.iter_mut() {
        let fallback = std::mem::take(&mut region.fallback);
        region.fallback = topo_sort(flat, fallback, &[]).expect("a region's own gates schedule");
    }
    match topo_sort(flat, ops, &regions) {
        Ok(ops) => Ok(FoldedNetlist { ops, regions }),
        // Only the regions among the stuck ops are worth disabling; the
        // gates around them are stuck because of the regions, not the
        // other way round.
        Err(stuck) => Err(stuck
            .into_iter()
            .filter_map(|op| match op {
                FlatOp::Region(index) => Some(regions[index as usize].region),
                _ => None,
            })
            .collect()),
    }
}

/// Which regions may be scheduled as one node.
///
/// A region is disqualified when skipping its gates would skip
/// something the rest of the circuit still needs:
///
/// - **A register the region holds but does not drive.** Its `d` comes
///   leaves that pin undriven. A register that *holds itself* — `d`
///   wired to its own `q`, which is how a constant is written in a
///   world with no constant sources — is fine: its `d` is driven by
///   its `q`, both evaluators load `q` from register state at the top
///   of every tick, and skipping the gates leaves it simply dead. That
///   distinction is what lets `std.mul16` be claimed at all; it carries
///   exactly one register and it is a constant zero.
/// - **A declared output nothing inside produces.** That is a port
///   wired straight through, so the value comes from outside and the
///   region does not compute it. (This is also what stops a constant's
///   `q` being a region output.)
/// - **No gates at all.** Pure wiring; there is nothing to replace.
/// - **No key.** A module that cannot be flattened on its own has no
///   digest, so nothing can be registered against it unambiguously.
fn atomic_regions(flat: &FlatCircuit, banked: &[bool]) -> Vec<bool> {
    let count = flat.intrinsics().len();
    let mut produced: Vec<Vec<NetId>> = vec![Vec::new(); count];
    let mut gate_count = vec![0_usize; count];
    for gate in flat.gates() {
        if let Some(region) = gate.region {
            produced[region as usize].push(gate.y);
            gate_count[region as usize] += 1;
        }
    }
    // A register the region holds but does not drive is one it cannot
    // be handed responsibility for. Its `d` comes from outside -- a
    // module input wired straight to a latch -- so a substitute given
    // the job of producing next state would be writing over a value
    // somebody else owns.
    //
    // A register that holds *itself* is neither: `d` is its own `q`,
    // which is loaded from register state whatever runs, so skipping
    // the gates leaves it simply dead. That is how a constant is
    // written where there are no constant sources, and it is what lets
    // `std.mul16` be claimed at all.
    // A register a delay bank claimed has no slot of its own: the bank
    // holds its value and publishes it. Nothing can be handed
    // responsibility for state it cannot address, so a region holding
    // one is not offered.
    let mut holds_banked = vec![false; count];
    for (index, reg) in flat.regs().iter().enumerate() {
        if let Some(region) = reg.region
            && banked.get(index).copied().unwrap_or(false)
        {
            holds_banked[region as usize] = true;
        }
    }
    let mut unowned_state = vec![false; count];
    for reg in flat.regs() {
        if let Some(region) = reg.region
            && reg.d != reg.q
            && !produced[region as usize].contains(&reg.d)
        {
            unowned_state[region as usize] = true;
        }
    }
    (0..count)
        .map(|index| {
            let decl = &flat.intrinsics()[index];
            decl.digest.is_some()
                && !unowned_state[index]
                && !holds_banked[index]
                && gate_count[index] > 0
                && decl.outputs.iter().all(|net| produced[index].contains(net))
        })
        .collect()
}

/// Deterministic Kahn re-sort of the mixed op list. On failure returns
/// the ops that could not be scheduled, so the caller can disable just
/// the regions among them rather than every region in the circuit.
///
/// Failure is only possible once regions are atomic --
/// the gate-level list is always schedulable, since folding only merges
/// gates that were already in an order.
fn topo_sort(
    flat: &FlatCircuit,
    ops: Vec<FlatOp>,
    regions: &[FoldedRegion],
) -> Result<Vec<FlatOp>, Vec<FlatOp>> {
    let mut producer: Vec<Option<u32>> = vec![None; flat.net_count as usize];
    for (index, op) in ops.iter().enumerate() {
        op.for_each_output(regions, |net| producer[net as usize] = Some(index as u32));
    }
    let mut indegree = vec![0_u32; ops.len()];
    let mut consumers: Vec<Vec<u32>> = vec![Vec::new(); ops.len()];
    for (index, op) in ops.iter().enumerate() {
        // Repeated inputs are counted twice and released twice, which
        // balances, so they need no special handling.
        op.for_each_input(regions, |net| {
            if let Some(prod) = producer[net as usize] {
                indegree[index] += 1;
                consumers[prod as usize].push(index as u32);
            }
        });
    }
    let mut queue: std::collections::VecDeque<u32> = (0..ops.len() as u32)
        .filter(|&index| indegree[index as usize] == 0)
        .collect();
    let mut order: Vec<u32> = Vec::with_capacity(ops.len());
    while let Some(index) = queue.pop_front() {
        order.push(index);
        for &consumer in &consumers[index as usize] {
            indegree[consumer as usize] -= 1;
            if indegree[consumer as usize] == 0 {
                queue.push_back(consumer);
            }
        }
    }
    if order.len() != ops.len() {
        let scheduled: Vec<bool> = {
            let mut seen = vec![false; ops.len()];
            for index in &order {
                seen[*index as usize] = true;
            }
            seen
        };
        return Err(ops
            .into_iter()
            .zip(scheduled)
            .filter(|(_, done)| !*done)
            .map(|(op, _)| op)
            .collect());
    }
    Ok(order.into_iter().map(|index| ops[index as usize]).collect())
}
struct Matcher<'a> {
    flat: &'a FlatCircuit,
    /// Gate indices consuming each net (one entry per distinct gate).
    consumers: Vec<Vec<u32>>,
    /// Net feeds a register `d` pin or a top-level output: it must
    /// survive folding as an op output or stay untouched.
    observed: Vec<bool>,
}

impl<'a> Matcher<'a> {
    fn new(flat: &'a FlatCircuit) -> Self {
        let mut consumers: Vec<Vec<u32>> = vec![Vec::new(); flat.net_count as usize];
        for (index, gate) in flat.gates.iter().enumerate() {
            consumers[gate.a as usize].push(index as u32);
            if gate.b != gate.a {
                consumers[gate.b as usize].push(index as u32);
            }
        }
        let mut observed = vec![false; flat.net_count as usize];
        for reg in &flat.regs {
            observed[reg.d as usize] = true;
        }
        for (_, net) in &flat.outputs {
            observed[*net as usize] = true;
        }
        // A region's declared outputs are observed too. Without this a
        // declared output that nothing outside happens to read could be
        // an interior net of an adder or XOR and be folded away, and
        // the region's fallback would then never assign the value the
        // region advertises. Nothing could see it today -- an
        // unconnected output is unconnected -- but the region would be
        // promising something it does not compute, and two regions
        // differing only in which interior net they expose would fold
        // to the same thing.
        for region in flat.intrinsics() {
            for net in &region.outputs {
                observed[*net as usize] = true;
            }
        }
        Self {
            flat,
            consumers,
            observed,
        }
    }

    fn gate(&self, index: u32) -> &FlatGate {
        &self.flat.gates[index as usize]
    }

    /// The other input of a 2-input gate given one input, if distinct.
    fn other_input(&self, index: u32, known: NetId) -> Option<NetId> {
        let gate = self.gate(index);
        if gate.a == known && gate.b != known {
            Some(gate.b)
        } else if gate.b == known && gate.a != known {
            Some(gate.a)
        } else {
            None
        }
    }

    fn has_inputs(&self, index: u32, x: NetId, y: NetId) -> bool {
        let gate = self.gate(index);
        (gate.a == x && gate.b == y) || (gate.a == y && gate.b == x)
    }

    /// A net internal to the fold: consumed only by the expected gates
    /// and not observed by registers or top outputs.
    fn internal(&self, net: NetId, expected: &[u32]) -> bool {
        if self.observed[net as usize] {
            return false;
        }
        let mut consumers: Vec<u32> = self.consumers[net as usize].clone();
        consumers.sort_unstable();
        let mut expected: Vec<u32> = expected.to_vec();
        expected.sort_unstable();
        expected.dedup();
        consumers == expected
    }

    /// A NAND whose output is immediately inverted: an AND.
    ///
    /// `inner = nand(a, b)`, `outer = nand(inner, inner)`. The inner
    /// net has to have no other consumer and be unobserved, or the
    /// value it carries is wanted in its own right and the pair cannot
    /// collapse -- exactly the condition the other folds use.
    fn match_and(&self, inner: usize, claimed: &[bool]) -> Option<(FoldedAnd, [usize; 2])> {
        let inner = inner as u32;
        let g = self.gate(inner);
        if g.a == g.b {
            // An inverter, not an AND's front half.
            return None;
        }
        let [outer] = self.consumers[g.y as usize][..] else {
            return None;
        };
        if claimed[outer as usize] || self.observed[g.y as usize] {
            return None;
        }
        let outer_gate = self.gate(outer);
        if outer_gate.a != g.y || outer_gate.b != g.y {
            return None;
        }
        Some((
            FoldedAnd {
                a: g.a,
                b: g.b,
                out: outer_gate.y,
            },
            [inner as usize, outer as usize],
        ))
    }

    /// Tries to match a full adder rooted at `n1` (the `nand(a, b)`
    /// gate); returns the fused op and the nine claimed gate indices.
    fn match_full_adder(&self, n1: usize, claimed: &[bool]) -> Option<(FoldedAdder, [usize; 9])> {
        let n1 = n1 as u32;
        let g1 = self.gate(n1);
        let (a, b) = (g1.a, g1.b);
        if a == b {
            return None;
        }

        // n1.y feeds exactly n2, n3, n9.
        let n1_consumers = &self.consumers[g1.y as usize];
        if n1_consumers.len() != 3 {
            return None;
        }
        let mut n2 = None;
        let mut n3 = None;
        for &index in n1_consumers {
            if self.has_inputs(index, a, g1.y) {
                n2 = Some(index);
            } else if self.has_inputs(index, b, g1.y) {
                n3 = Some(index);
            }
        }
        let (n2, n3) = (n2?, n3?);
        if n2 == n3 {
            return None;
        }
        let n9 = *n1_consumers
            .iter()
            .find(|&&index| index != n2 && index != n3)?;

        // n2.y and n3.y both feed only n4.
        let [n4] = self.consumers[self.gate(n2).y as usize][..] else {
            return None;
        };
        if self.consumers[self.gate(n3).y as usize] != [n4] {
            return None;
        }
        if !self.has_inputs(n4, self.gate(n2).y, self.gate(n3).y) {
            return None;
        }

        // n4.y feeds n5 (with cin) and n6 (with n5.y).
        let n4y = self.gate(n4).y;
        let [first, second] = self.consumers[n4y as usize][..] else {
            return None;
        };
        let mut resolved = None;
        for (n5, n6) in [(first, second), (second, first)] {
            let Some(cin) = self.other_input(n5, n4y) else {
                continue;
            };
            if self.has_inputs(n6, n4y, self.gate(n5).y) {
                resolved = Some((n5, n6, cin));
                break;
            }
        }
        let (n5, n6, cin) = resolved?;
        let n5y = self.gate(n5).y;

        // n5.y feeds exactly n6, n7, n9; n9 = nand(n5.y, n1.y).
        let n5_consumers = &self.consumers[n5y as usize];
        if n5_consumers.len() != 3 || !n5_consumers.contains(&n6) || !n5_consumers.contains(&n9) {
            return None;
        }
        let n7 = *n5_consumers
            .iter()
            .find(|&&index| index != n6 && index != n9)?;
        if !self.has_inputs(n7, cin, n5y) || !self.has_inputs(n9, n5y, g1.y) {
            return None;
        }

        // n6.y and n7.y both feed only n8 = sum.
        let [n8] = self.consumers[self.gate(n6).y as usize][..] else {
            return None;
        };
        if self.consumers[self.gate(n7).y as usize] != [n8] {
            return None;
        }
        if !self.has_inputs(n8, self.gate(n6).y, self.gate(n7).y) {
            return None;
        }

        let gates = [n1, n2, n3, n4, n5, n6, n7, n8, n9];
        let mut distinct = gates.to_vec();
        distinct.sort_unstable();
        distinct.dedup();
        if distinct.len() != gates.len() || gates.iter().any(|&g| claimed[g as usize]) {
            return None;
        }

        // Interior nets stay strictly inside the fold.
        let interior_ok = self.internal(g1.y, &[n2, n3, n9])
            && self.internal(self.gate(n2).y, &[n4])
            && self.internal(self.gate(n3).y, &[n4])
            && self.internal(n4y, &[n5, n6])
            && self.internal(n5y, &[n6, n7, n9])
            && self.internal(self.gate(n6).y, &[n8])
            && self.internal(self.gate(n7).y, &[n8]);
        if !interior_ok {
            return None;
        }

        Some((
            FoldedAdder {
                a,
                b,
                cin,
                sum: self.gate(n8).y,
                cout: self.gate(n9).y,
            },
            gates.map(|g| g as usize),
        ))
    }

    /// Tries to match a 4-NAND XOR rooted at `n1` (the `nand(a, b)`
    /// gate); returns the fused op and the four claimed gate indices.
    fn match_xor(&self, n1: usize, claimed: &[bool]) -> Option<(FoldedXor, [usize; 4])> {
        let n1 = n1 as u32;
        let g1 = self.gate(n1);
        let (a, b) = (g1.a, g1.b);
        if a == b {
            return None;
        }

        // n1.y feeds exactly n2 = nand(a, n1.y) and n3 = nand(b, n1.y).
        let [first, second] = self.consumers[g1.y as usize][..] else {
            return None;
        };
        let mut n2 = None;
        let mut n3 = None;
        for index in [first, second] {
            if self.has_inputs(index, a, g1.y) {
                n2 = Some(index);
            } else if self.has_inputs(index, b, g1.y) {
                n3 = Some(index);
            }
        }
        let (n2, n3) = (n2?, n3?);
        if n2 == n3 {
            return None;
        }

        // n2.y and n3.y both feed only n4 = nand(n2.y, n3.y).
        let [n4] = self.consumers[self.gate(n2).y as usize][..] else {
            return None;
        };
        if self.consumers[self.gate(n3).y as usize] != [n4] {
            return None;
        }
        if !self.has_inputs(n4, self.gate(n2).y, self.gate(n3).y) {
            return None;
        }

        let gates = [n1, n2, n3, n4];
        let mut distinct = gates.to_vec();
        distinct.sort_unstable();
        distinct.dedup();
        if distinct.len() != gates.len() || gates.iter().any(|&g| claimed[g as usize]) {
            return None;
        }

        let interior_ok = self.internal(g1.y, &[n2, n3])
            && self.internal(self.gate(n2).y, &[n4])
            && self.internal(self.gate(n3).y, &[n4]);
        if !interior_ok {
            return None;
        }

        Some((
            FoldedXor {
                a,
                b,
                out: self.gate(n4).y,
            },
            gates.map(|g| g as usize),
        ))
    }
}
