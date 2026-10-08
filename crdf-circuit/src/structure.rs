//! **Retained structure**: how a module was built, kept beside it.
//!
//! The library is built by imperative
//! emission — `for i in 0..16 { m.instance(..) }` writing cells into a
//! builder — which *destroys* the structure the author knew. Everything
//! downstream then tries to recover it: a schematic viewer guesses at
//! which boxes are lanes of one operation (and gets eleven per cent), and
//! a person reading an unstructured multiplier sees hundreds of cells where
//! the author saw repeated rows.
//!
//! The fix is not prettier iteration. Helpers that merely loop more
//! neatly emit exactly the same forgotten netlist. What is needed is a
//! **term that survives elaboration**: say `scan(24, full_adder)`, emit
//! precisely the cells that were emitted before, and *keep the term*, so
//! that what the author knew is a fact anyone downstream can read rather
//! than a pattern they must rediscover.
//!
//! ## What this is not
//!
//! - **Not semantics.** No `CellKind`, no evaluator instruction, no third
//!   axiom. The netlist remains the canonical encoding; a term records
//!   only *how it came to be*, and every evaluator ignores it.
//! - **Not recursive instantiation.**
//!   A term's parameters determine a **finite** elaboration, and a body
//!   may not reference the definition containing it. An elaborator that
//!   walks a finite structure is irrelevant to that rule; what the rule
//!   excludes is "keep instantiating until something stops me".
//! - **Not a module boundary each.** Abstraction is free in gates and
//!   registers and *not* free in hierarchy depth, identity or
//!   persistence, so a term is provenance beside the cells, not another
//!   level of nesting around them.

use crate::model::{CellId, CellKind, Endpoint, ModuleBuilder};

/// A group of cells the author built as **one repetition**.
///
/// The thing a viewer needs in order to draw `FULL ADDER ×24` and be
/// right, rather than to infer it and be eleven per cent right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lane {
    /// What the repetition is, for a breadcrumb: `"sum"`, `"partial"`.
    pub label: String,
    /// The body every member instantiates. All members share it — that
    /// is what makes them one repetition rather than a coincidence.
    pub body: CellKind,
    /// One cell per iteration, in elaboration order, so `cells[7]` is
    /// lane 7 and a drill-down can say which lane it entered.
    pub cells: Vec<CellId>,
}

/// Elaborates `count` instances of `body` **side by side**, with nothing
/// threaded between them, and returns the lane.
///
/// The third shape, and the commonest. A scan is a chain and a fold is a
/// tree; this is neither — sixteen lanes of one operation, each minding
/// its own bits. It is what "apply this to every bit of the word" builds,
/// and it is most of what makes a big module unreadable: a bank of
/// independent muxes draws one box per bit although no lane depends
/// on another.
///
/// **A separate variant rather than a chain with the threading left
/// out.** The payload is identical — a label, a body, the cells — and
/// only the topology differs, which is exactly what a reader needs to be
/// able to trust. Encoding a map as a `Chain` whose carry happens to be
/// absent would weaken what `Chain` promises and make every consumer
/// check an implicit condition to find out which it really had.
///
/// The callback receives the builder, the index and the cell, and cannot
/// change how many instances exist or which body they share — the same
/// property that keeps [`scan`]'s term honest.
pub fn map(
    m: &mut ModuleBuilder,
    label: &str,
    count: usize,
    body: CellKind,
    mut wire: impl FnMut(&mut ModuleBuilder, usize, CellId),
) -> Lane {
    let mut cells = Vec::with_capacity(count);
    for index in 0..count {
        let cell = m.cell(body);
        wire(m, index, cell);
        cells.push(cell);
    }
    let lane = Lane {
        label: label.to_string(),
        body,
        cells,
    };
    // Recorded as it is built, so a caller cannot forget to keep what it
    // just stated.
    m.record(Structure::Map(lane.clone()));
    lane
}

/// Records a map over cells the caller **already built**, without
/// emitting anything.
///
/// The escape hatch for a loop that cannot be rewritten. [`map`] creates
/// its instances consecutively, which is right when a loop emits one
/// kind — and impossible when it emits several interleaved gate
/// helpers. Rewriting such a
/// loop into two `map` calls would reorder the cells, and **emission
/// order is persistent identity here**: a canonical cell id is a UUIDv5
/// of `<module>/cell/<ordinal>`, and saved files use those ids as
/// resource identities that conflict on merge. So the loop stays exactly
/// as it is and the term is recorded beside it.
///
/// **This is the weaker constructor and it is worth saying why.** `map`
/// is honest by construction: it emits what it describes, so the two
/// cannot disagree. Here the caller supplies the cells, so it *could*
/// name cells it did not build, or cells of mixed kinds. Everything that
/// can be checked is checked — the cells exist, they are all instances
/// of `body`, and none is named twice — and it panics rather than
/// recording a term that is not true, because a library that lies about
/// its own structure should fail while it is being built and not later
/// in a viewer.
///
/// What cannot be checked is the one thing that makes it a *repetition*
/// rather than a set: that the author meant these cells as one. That
/// stays the caller's word.
pub fn record_map(m: &mut ModuleBuilder, label: &str, body: CellKind, cells: Vec<CellId>) -> Lane {
    let mut seen = std::collections::HashSet::with_capacity(cells.len());
    for cell in &cells {
        assert!(
            seen.insert(*cell),
            "record_map({label:?}): cell named twice; a cell drawn as two \
             boxes is a cell counted twice"
        );
        assert_eq!(
            m.cell_kind(*cell),
            Some(body),
            "record_map({label:?}): the term claims a cell that is not of \
             its declared body"
        );
    }
    let lane = Lane {
        label: label.to_string(),
        body,
        cells,
    };
    m.record(Structure::Map(lane.clone()));
    lane
}

/// Records a chain over cells the caller **already built**.
///
/// [`record_map`]'s counterpart, and needed for the same reason: a loop
/// that emits two kinds turn and turn about cannot be rewritten into
/// two constructors without reordering its cells, and order is
/// persistent identity here. A conditional two's-complement negate
/// emits an XOR and a full adder per bit, and the full adders thread a
/// carry — so they are a chain, and the XORs beside them are a map, and
/// neither may move.
///
/// The same checks as `record_map`, for the same reason: this
/// constructor can be lied to where [`scan`] cannot, so what can be
/// verified is verified and a false term is a panic rather than
/// something a viewer discovers later.
pub fn record_chain(
    m: &mut ModuleBuilder,
    label: &str,
    body: CellKind,
    cells: Vec<CellId>,
) -> Lane {
    let mut seen = std::collections::HashSet::with_capacity(cells.len());
    for cell in &cells {
        assert!(
            seen.insert(*cell),
            "record_chain({label:?}): cell named twice; a cell drawn as \
             two boxes is a cell counted twice"
        );
        assert_eq!(
            m.cell_kind(*cell),
            Some(body),
            "record_chain({label:?}): the term claims a cell that is not \
             of its declared body"
        );
    }
    let lane = Lane {
        label: label.to_string(),
        body,
        cells,
    };
    m.record(Structure::Chain(lane.clone()));
    lane
}

/// Elaborates `count` instances of `body` in a chain, threading one
/// value from each to the next, and returns the lane together with the
/// value leaving the last.
///
/// This is the shape a ripple carry has, and a running sum, and an
/// accumulator: *scan* in the usual sense. The two callbacks are what
/// differs between uses — `wire` connects iteration `i`'s own inputs and
/// receives the incoming carry, `carry` says which pin the outgoing one
/// leaves by — and neither can change how many instances exist or which
/// body they share, which is what keeps the term an honest description.
pub fn scan(
    m: &mut ModuleBuilder,
    label: &str,
    count: usize,
    body: CellKind,
    carry_in: Endpoint,
    mut wire: impl FnMut(&mut ModuleBuilder, usize, CellId, Endpoint),
    carry: impl Fn(CellId) -> Endpoint,
) -> (Lane, Endpoint) {
    let mut cells = Vec::with_capacity(count);
    let mut running = carry_in;
    for index in 0..count {
        let cell = m.cell(body);
        wire(m, index, cell, running);
        running = carry(cell);
        cells.push(cell);
    }
    let lane = Lane {
        label: label.to_string(),
        body,
        cells,
    };
    // Recorded on the module as it is built, so a caller cannot forget
    // to keep what it just stated.
    m.record(Structure::Chain(lane.clone()));
    (lane, running)
}

/// A balanced combining tree: what the author built as **one fold**.
///
/// The second shape after [`Lane`], and a different one: a scan is a
/// chain of `n`, a tree is `log n` levels that halve. Both are
/// repetitions the author knew and the netlist forgot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    /// What is being folded, for a breadcrumb: `"select"`.
    pub label: String,
    /// The body every node instantiates.
    pub body: CellKind,
    /// One entry per level, widest first: `levels[0]` holds `n/2` nodes
    /// and the last holds one. A viewer can draw `MUX ×8`, `MUX ×4`, … or
    /// collapse the whole tree to a single box, and either is honest
    /// because both are what the author wrote.
    pub levels: Vec<Vec<CellId>>,
}

/// Elaborates a balanced fold over `leaves`, pairing neighbours at each
/// level until one value remains, and returns the tree with it.
///
/// `leaves.len()` must be a power of two, so every level halves exactly
/// and there is no odd-one-out policy to invent — the same reason
/// `index` addresses `2^k` elements.
pub fn tree(
    m: &mut ModuleBuilder,
    label: &str,
    leaves: &[Endpoint],
    body: CellKind,
    mut combine: impl FnMut(&mut ModuleBuilder, usize, CellId, Endpoint, Endpoint),
    out: impl Fn(CellId) -> Endpoint,
) -> (Tree, Endpoint) {
    assert!(
        leaves.len().is_power_of_two() && !leaves.is_empty(),
        "a balanced fold halves exactly"
    );
    let mut levels: Vec<Vec<CellId>> = Vec::new();
    let mut layer: Vec<Endpoint> = leaves.to_vec();
    let mut level = 0;
    while layer.len() > 1 {
        let mut cells = Vec::with_capacity(layer.len() / 2);
        let mut next = Vec::with_capacity(layer.len() / 2);
        for pair in layer.as_chunks::<2>().0 {
            let cell = m.cell(body);
            combine(m, level, cell, pair[0], pair[1]);
            next.push(out(cell));
            cells.push(cell);
        }
        levels.push(cells);
        layer = next;
        level += 1;
    }
    let term = Tree {
        label: label.to_string(),
        body,
        levels,
    };
    m.record(Structure::Fold(term.clone()));
    (term, layer[0])
}

/// A retained structural term, whatever its shape.
///
/// What a library carries beside a module so that everything downstream
/// — a schematic viewer first — is *told* how it was built instead of
/// having to work it out from the flattened result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Structure {
    /// `n` side by side, independent ([`map`]).
    Map(Lane),
    /// A chain of `n`, threading one value ([`scan`]).
    Chain(Lane),
    /// A fold halving a layer ([`tree`]).
    Fold(Tree),
}

impl Structure {
    /// Every cell the term accounts for, so a viewer can ask "is this box
    /// part of a repetition" without knowing which shape it is.
    pub fn cells(&self) -> Vec<CellId> {
        match self {
            Structure::Map(lane) | Structure::Chain(lane) => lane.cells.clone(),
            Structure::Fold(tree) => tree.levels.iter().flatten().copied().collect(),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Structure::Map(lane) | Structure::Chain(lane) => &lane.label,
            Structure::Fold(tree) => &tree.label,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::flatten;
    use crate::interp::FlatSimulator;
    use crate::model::ModuleLibrary;
    use crate::stdlib::{Stdlib, build_ripple_adder, build_ripple_adder_scanned};

    pub(super) const WIDTH: usize = 8;

    /// `count` xor gates side by side: `a.i`, `b.i` → `y.i`.
    ///
    /// Built with a hand-written loop, which is how every wall in the
    /// library is built today.
    fn bitwise_bank_looped(name: &str, count: usize, stdlib: &Stdlib) -> crate::model::Module {
        let ports = crate::stdlib::gate_ports(&stdlib.library, stdlib.xor_gate);
        let mut m = ModuleBuilder::new_canonical(name.to_string());
        let a: Vec<_> = (0..count).map(|i| m.input(format!("a.{i}"))).collect();
        let b: Vec<_> = (0..count).map(|i| m.input(format!("b.{i}"))).collect();
        let y: Vec<_> = (0..count).map(|i| m.output(format!("y.{i}"))).collect();
        for i in 0..count {
            let cell = m.instance(stdlib.xor_gate);
            m.wire(Endpoint::module(a[i]), Endpoint::inner(cell, ports.a));
            m.wire(Endpoint::module(b[i]), Endpoint::inner(cell, ports.b));
            m.wire(Endpoint::inner(cell, ports.y), Endpoint::module(y[i]));
        }
        m.finish()
    }

    /// The same bank, said as a map.
    pub(super) fn bitwise_bank_mapped(
        name: &str,
        count: usize,
        stdlib: &Stdlib,
    ) -> crate::model::Module {
        let ports = crate::stdlib::gate_ports(&stdlib.library, stdlib.xor_gate);
        let mut m = ModuleBuilder::new_canonical(name.to_string());
        let a: Vec<_> = (0..count).map(|i| m.input(format!("a.{i}"))).collect();
        let b: Vec<_> = (0..count).map(|i| m.input(format!("b.{i}"))).collect();
        let y: Vec<_> = (0..count).map(|i| m.output(format!("y.{i}"))).collect();
        map(
            &mut m,
            "bit",
            count,
            CellKind::Instance {
                module: stdlib.xor_gate,
            },
            |m, i, cell| {
                m.wire(Endpoint::module(a[i]), Endpoint::inner(cell, ports.a));
                m.wire(Endpoint::module(b[i]), Endpoint::inner(cell, ports.b));
                m.wire(Endpoint::inner(cell, ports.y), Endpoint::module(y[i]));
            },
        );
        m.finish()
    }

    /// **`record_map` describes an interleaved loop without moving it.**
    ///
    /// The case `map` cannot serve: a loop emitting two kinds turn and
    /// turn about. Rewriting it into two `map` calls would group the
    /// cells and reorder them, and order is persistent identity here.
    /// So the loop is left alone and two terms are recorded over it.
    #[test]
    fn record_map_describes_a_loop_it_did_not_build() {
        let stdlib = Stdlib::build();
        let and = crate::stdlib::gate_ports(&stdlib.library, stdlib.and_gate);
        let or = crate::stdlib::gate_ports(&stdlib.library, stdlib.or_gate);

        let mut m = ModuleBuilder::new_canonical("t.interleaved".to_string());
        let a: Vec<_> = (0..WIDTH).map(|i| m.input(format!("a.{i}"))).collect();
        let b: Vec<_> = (0..WIDTH).map(|i| m.input(format!("b.{i}"))).collect();
        let y: Vec<_> = (0..WIDTH).map(|i| m.output(format!("y.{i}"))).collect();

        // and, or, and, or … exactly as it was written.
        let mut ands = Vec::new();
        let mut ors = Vec::new();
        for i in 0..WIDTH {
            let x = m.instance(stdlib.and_gate);
            m.wire(Endpoint::module(a[i]), Endpoint::inner(x, and.a));
            m.wire(Endpoint::module(b[i]), Endpoint::inner(x, and.b));
            ands.push(x);
            let o = m.instance(stdlib.or_gate);
            m.wire(Endpoint::inner(x, and.y), Endpoint::inner(o, or.a));
            m.wire(Endpoint::module(b[i]), Endpoint::inner(o, or.b));
            m.wire(Endpoint::inner(o, or.y), Endpoint::module(y[i]));
            ors.push(o);
        }
        let interleaved: Vec<_> = m.finish().cells.iter().map(|c| c.kind).collect();

        // Rebuild identically and describe it.
        let mut m = ModuleBuilder::new_canonical("t.interleaved".to_string());
        let a: Vec<_> = (0..WIDTH).map(|i| m.input(format!("a.{i}"))).collect();
        let b: Vec<_> = (0..WIDTH).map(|i| m.input(format!("b.{i}"))).collect();
        let y: Vec<_> = (0..WIDTH).map(|i| m.output(format!("y.{i}"))).collect();
        let mut ands = Vec::new();
        let mut ors = Vec::new();
        for i in 0..WIDTH {
            let x = m.instance(stdlib.and_gate);
            m.wire(Endpoint::module(a[i]), Endpoint::inner(x, and.a));
            m.wire(Endpoint::module(b[i]), Endpoint::inner(x, and.b));
            ands.push(x);
            let o = m.instance(stdlib.or_gate);
            m.wire(Endpoint::inner(x, and.y), Endpoint::inner(o, or.a));
            m.wire(Endpoint::module(b[i]), Endpoint::inner(o, or.b));
            m.wire(Endpoint::inner(o, or.y), Endpoint::module(y[i]));
            ors.push(o);
        }
        record_map(
            &mut m,
            "mask",
            CellKind::Instance {
                module: stdlib.and_gate,
            },
            ands,
        );
        record_map(
            &mut m,
            "merge",
            CellKind::Instance {
                module: stdlib.or_gate,
            },
            ors,
        );
        let described = m.finish();

        assert_eq!(
            described.cells.iter().map(|c| c.kind).collect::<Vec<_>>(),
            interleaved,
            "describing the loop moved a cell; and, or, and, or is exactly \
             the order that has to survive"
        );
        assert_eq!(described.structures.len(), 2, "two honest repetitions");
    }

    /// **A bank of registers is a repetition too.**
    ///
    /// `Lane::body` was a `ModuleId`, so sixteen `m.reg()` calls could
    /// not be described at all — and a register bank is not a corner
    /// case here: wide state machines can hold dozens, and on screen
    /// they become the tallest thing in the frame, one loose box each.
    ///
    /// Wrapping each in a module would have been the wrong fix: a
    /// register is already an immediate cell, and a boundary per bit
    /// would change hierarchy depth, identity and instance paths — the
    /// exact costs retained structure exists to avoid.
    #[test]
    fn a_register_bank_can_be_described() {
        let stdlib = Stdlib::build();
        let mut m = ModuleBuilder::new_canonical("t.regbank".to_string());
        let d: Vec<_> = (0..WIDTH).map(|i| m.input(format!("d.{i}"))).collect();
        let q: Vec<_> = (0..WIDTH).map(|i| m.output(format!("q.{i}"))).collect();

        let lane = map(
            &mut m,
            "state",
            WIDTH,
            CellKind::Reg { init: false },
            |m, i, cell| {
                m.wire(Endpoint::module(d[i]), Endpoint::reg_d(cell));
                m.wire(Endpoint::reg_q(cell), Endpoint::module(q[i]));
            },
        );
        let module = m.finish();

        assert_eq!(lane.cells.len(), WIDTH, "one register per bit");
        assert_eq!(lane.body, CellKind::Reg { init: false });
        assert_eq!(module.structures.len(), 1, "one bank, one term");
        assert!(
            module
                .cells
                .iter()
                .all(|c| c.kind == CellKind::Reg { init: false }),
            "the bank is registers, not instances of anything"
        );

        // And it survives a save as what it is. The body has no module
        // to name, so a format that could only write a module IRI would
        // have lost it -- silently, since the term would still parse as
        // *something*.
        let (library, id) = library_with(module, &stdlib);
        let graph = crate::crdf_io::asset_to_rdf(&crate::model::CircuitAsset::new(id, 1, library))
            .expect("writes");
        let back = crate::crdf_io::asset_from_rdf(&graph).expect("reads");
        match back
            .library
            .get(id)
            .expect("the module")
            .structures
            .as_slice()
        {
            [Structure::Map(read_back)] => assert_eq!(*read_back, lane, "the bank came back whole"),
            other => panic!("expected one map over registers, got {other:?}"),
        }
    }

    /// **It refuses a cell that is not its body.**
    ///
    /// `map` cannot be lied to — it emits what it describes. This one
    /// can, so what can be checked is checked, and a library that would
    /// have lied about itself fails while it is being built.
    #[test]
    #[should_panic(expected = "not of its declared body")]
    fn record_map_refuses_a_cell_of_another_kind() {
        let stdlib = Stdlib::build();
        let mut m = ModuleBuilder::new_canonical("t.wrong_body".to_string());
        let good = m.instance(stdlib.and_gate);
        let other = m.instance(stdlib.or_gate);
        record_map(
            &mut m,
            "mixed",
            CellKind::Instance {
                module: stdlib.and_gate,
            },
            vec![good, other],
        );
    }

    /// And a cell it never built at all.
    #[test]
    #[should_panic(expected = "not of its declared body")]
    fn record_map_refuses_a_cell_that_is_not_there() {
        let stdlib = Stdlib::build();
        let mut m = ModuleBuilder::new_canonical("t.absent".to_string());
        let good = m.instance(stdlib.and_gate);
        record_map(
            &mut m,
            "ghost",
            CellKind::Instance {
                module: stdlib.and_gate,
            },
            vec![good, CellId::new_random()],
        );
    }

    /// And the same cell twice, which would draw one cell as two boxes.
    #[test]
    #[should_panic(expected = "named twice")]
    fn record_map_refuses_a_repeated_cell() {
        let stdlib = Stdlib::build();
        let mut m = ModuleBuilder::new_canonical("t.twice".to_string());
        let cell = m.instance(stdlib.and_gate);
        record_map(
            &mut m,
            "double",
            CellKind::Instance {
                module: stdlib.and_gate,
            },
            vec![cell, cell],
        );
    }

    /// **Saying `map` emits the circuit the loop emitted.**
    ///
    /// The claim the whole idea rests on, for the third shape. If a term
    /// changed the netlist it would be a rewrite rather than a
    /// description, and every gate count and every saved patch would be
    /// at stake.
    #[test]
    fn a_mapped_bank_is_the_same_circuit() {
        let stdlib = Stdlib::build();
        let looped = bitwise_bank_looped("t.loop_bank", WIDTH, &stdlib);
        let mapped = bitwise_bank_mapped("t.map_bank", WIDTH, &stdlib);

        assert_eq!(looped.cells.len(), mapped.cells.len(), "same cells");
        assert_eq!(looped.wires.len(), mapped.wires.len(), "same wires");
        assert_eq!(looped.ports.len(), mapped.ports.len(), "same boundary");
        assert!(
            looped.structures.is_empty(),
            "the loop states nothing, which is the problem being fixed"
        );

        let (lib_a, id_a) = library_with(looped, &stdlib);
        let (lib_b, id_b) = library_with(mapped, &stdlib);
        let flat_a = flatten(id_a, &lib_a).expect("flattens");
        let flat_b = flatten(id_b, &lib_b).expect("flattens");
        assert_eq!(
            (flat_a.gate_count(), flat_a.reg_count()),
            (flat_b.gate_count(), flat_b.reg_count()),
            "a retained term may not cost a gate"
        );
    }

    /// And it still xors. Counts can match while the wiring differs, so
    /// the only proof is running it.
    #[test]
    fn a_mapped_bank_computes_the_same_bits() {
        let stdlib = Stdlib::build();
        let mapped = bitwise_bank_mapped("t.map_bank2", WIDTH, &stdlib);
        let (library, id) = library_with(mapped, &stdlib);
        let flat = flatten(id, &library).expect("flattens");
        let mut sim = FlatSimulator::new(&flat);

        for (a, b) in [(0_u32, 0_u32), (0xff, 0), (0, 0xff), (0xaa, 0x55), (99, 99)] {
            for bit in 0..WIDTH {
                assert!(sim.set_input(&format!("a.{bit}"), (a >> bit) & 1 == 1));
                assert!(sim.set_input(&format!("b.{bit}"), (b >> bit) & 1 == 1));
            }
            sim.tick();
            let got = (0..WIDTH).fold(0_u32, |word, bit| {
                word | (u32::from(sim.output(&format!("y.{bit}")).expect("y")) << bit)
            });
            assert_eq!(got, (a ^ b) & 0xff, "{a:#x} ^ {b:#x}");
        }
    }

    /// **The term names every cell it built, and only those.**
    ///
    /// A term that under-claims leaves boxes unfolded, which is merely
    /// disappointing. One that over-claims marks a cell folded that no
    /// box stands for, and the viewer drops its wires silently — the
    /// failure this whole mechanism exists to avoid.
    #[test]
    fn a_map_term_accounts_for_exactly_its_cells() {
        let stdlib = Stdlib::build();
        let mapped = bitwise_bank_mapped("t.map_bank3", WIDTH, &stdlib);
        assert_eq!(mapped.structures.len(), 1, "one repetition, one term");
        let term = &mapped.structures[0];
        assert!(
            matches!(term, Structure::Map(_)),
            "a bank of independent gates is a map, not a chain or a fold"
        );

        let named = term.cells();
        assert_eq!(named.len(), WIDTH, "one cell per lane");
        let present: std::collections::HashSet<_> = mapped.cells.iter().map(|c| c.id).collect();
        for cell in &named {
            assert!(
                present.contains(cell),
                "the term names a cell that is not there"
            );
        }
        let unique: std::collections::HashSet<_> = named.iter().copied().collect();
        assert_eq!(unique.len(), named.len(), "a cell may not be named twice");
        assert_eq!(
            unique.len(),
            present.len(),
            "every cell of the bank belongs to the repetition"
        );
    }

    fn library_with(
        module: crate::model::Module,
        stdlib: &Stdlib,
    ) -> (ModuleLibrary, crate::model::ModuleId) {
        let id = module.id;
        let mut library = stdlib.library.clone();
        library.insert(module);
        (library, id)
    }

    /// **Declarative construction emits today's artefact.**
    ///
    /// The first of the three claims retained structure rests on. If
    /// saying `scan` gave a different circuit from writing the loop, the
    /// idea would be a rewrite rather than a description, and every gate
    /// count and every saved circuit would be at stake.
    #[test]
    fn a_scanned_adder_is_the_same_circuit() {
        let stdlib = Stdlib::build();
        let loop_built = build_ripple_adder("t.loop", WIDTH, stdlib.full_adder, &stdlib.library);
        let (scanned, _) =
            build_ripple_adder_scanned("t.scan", WIDTH, stdlib.full_adder, &stdlib.library);

        assert_eq!(loop_built.cells.len(), scanned.cells.len(), "same cells");
        assert_eq!(loop_built.wires.len(), scanned.wires.len(), "same wires");
        let names = |m: &crate::model::Module| {
            let mut n: Vec<String> = m.ports.iter().map(|p| p.name.clone()).collect();
            n.sort();
            n
        };
        assert_eq!(names(&loop_built), names(&scanned), "same boundary");

        let (lib_a, id_a) = library_with(loop_built, &stdlib);
        let (lib_b, id_b) = library_with(scanned, &stdlib);
        let flat_a = flatten(id_a, &lib_a).expect("flattens");
        let flat_b = flatten(id_b, &lib_b).expect("flattens");
        assert_eq!(
            (flat_a.gate_count(), flat_a.reg_count()),
            (flat_b.gate_count(), flat_b.reg_count()),
            "a retained term may not cost a gate"
        );
    }

    /// And it adds. Structural sameness is necessary and not sufficient:
    /// two circuits can have identical counts and different wiring, so
    /// the spike is only proved by running both.
    #[test]
    fn a_scanned_adder_computes_the_same_sums() {
        let stdlib = Stdlib::build();
        let (scanned, _) =
            build_ripple_adder_scanned("t.scan2", WIDTH, stdlib.full_adder, &stdlib.library);
        let (library, id) = library_with(scanned, &stdlib);
        let flat = flatten(id, &library).expect("flattens");
        let mut sim = FlatSimulator::new(&flat);

        for (a, b) in [
            (0_u32, 0_u32),
            (1, 1),
            (200, 55),
            (255, 1),
            (170, 85),
            (99, 99),
        ] {
            for bit in 0..WIDTH {
                assert!(sim.set_input(&format!("a.{bit}"), (a >> bit) & 1 == 1));
                assert!(sim.set_input(&format!("b.{bit}"), (b >> bit) & 1 == 1));
            }
            assert!(sim.set_input("cin", false));
            sim.tick();
            let sum = (0..WIDTH).fold(0_u32, |word, bit| {
                word | (u32::from(sim.output(&format!("sum.{bit}")).expect("sum")) << bit)
            });
            let carry = u32::from(sim.output("cout").expect("cout"));
            assert_eq!(sum | (carry << WIDTH), a + b, "{a} + {b}");
        }
    }

    /// **The repetition is retained**, which is the claim the other two
    /// exist to make worth having.
    ///
    /// A viewer holding this does not infer that the eight full adders
    /// are one carry chain — it is told, by the author, exactly. That is
    /// the difference between folding with certainty and the eleven per
    /// cent a heuristic managed.
    #[test]
    fn the_repetition_survives_elaboration() {
        let stdlib = Stdlib::build();
        let (module, lane) =
            build_ripple_adder_scanned("t.scan3", WIDTH, stdlib.full_adder, &stdlib.library);

        assert_eq!(lane.label, "sum");
        assert_eq!(
            lane.body,
            CellKind::Instance {
                module: stdlib.full_adder
            },
            "every member shares a body"
        );
        assert_eq!(lane.cells.len(), WIDTH, "one member per iteration");

        // The lane names cells that are really there, in order, and names
        // every one of them: a term that described cells the module does
        // not have, or missed some it does, would be worse than none.
        let present: Vec<_> = module.cells.iter().map(|c| c.id).collect();
        assert!(
            lane.cells.iter().all(|cell| present.contains(cell)),
            "the term names only cells that exist"
        );
        assert_eq!(
            lane.cells.len(),
            module.cells.len(),
            "and every cell of this module belongs to the scan"
        );
    }
}

#[cfg(test)]
mod tree_tests {
    use crate::compile::flatten;
    use crate::interp::FlatSimulator;
    use crate::model::CellKind;
    use crate::stdlib::{Stdlib, build_select16_plain, build_select16_treed};

    /// The second construction case (`tree`), and the one that settles what was
    /// missing.
    ///
    /// `build_select16`'s loop already *looked* combinatorial — halve the
    /// layer, pair the neighbours, repeat. If saying `tree` changed the
    /// circuit, retention would be a rewrite. It does not: same cells,
    /// same wires, same counts, and it still selects. So what the loop
    /// lacked was never a nicer way to iterate. It was that nobody
    /// **kept** what the iteration meant.
    #[test]
    fn a_treed_selector_is_the_same_circuit_and_still_selects() {
        let stdlib = Stdlib::build();
        let (treed, term) = build_select16_treed("t.treed", stdlib.mux, &stdlib.library);
        let plain = build_select16_plain("t.plain", stdlib.mux, &stdlib.library);

        assert_eq!(treed.cells.len(), plain.cells.len(), "same cells");
        assert_eq!(treed.wires.len(), plain.wires.len(), "same wires");

        let id = treed.id;
        let mut library = stdlib.library.clone();
        library.insert(treed);
        let flat = flatten(id, &library).expect("flattens");
        let mut sim = FlatSimulator::new(&flat);

        // A distinct value per slot, so a tree wired to the wrong
        // neighbour picks a demonstrably wrong one rather than a
        // coincidentally equal one.
        for chosen in 0..16_usize {
            for slot in 0..16 {
                assert!(sim.set_input(&format!("v.{slot}"), slot == chosen));
            }
            for bit in 0..4 {
                assert!(sim.set_input(&format!("sel.{bit}"), (chosen >> bit) & 1 == 1));
            }
            sim.tick();
            assert!(
                sim.output("y").expect("the selected value"),
                "index {chosen} must pick slot {chosen}"
            );
        }

        // And the fold is retained: four levels halving 8, 4, 2, 1.
        assert_eq!(term.label, "select");
        assert_eq!(term.body, CellKind::Instance { module: stdlib.mux });
        assert_eq!(
            term.levels.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![8, 4, 2, 1],
            "a balanced fold over sixteen leaves"
        );
        assert_eq!(
            term.levels.iter().map(Vec::len).sum::<usize>(),
            15,
            "and every mux belongs to a level"
        );
    }
}

#[cfg(test)]
mod library_tests {
    use super::*;
    use crate::stdlib::Stdlib;

    /// **The library's real modules now carry their terms**, which is what
    /// makes the two spikes worth having: a viewer reads `Stdlib` and is
    /// told, rather than reading a netlist and guessing.
    ///
    /// The ids and the circuits are unchanged — the retained builders emit
    /// what the loops emitted, under the same canonical names — so nothing
    /// saved, compiled or running is affected. Only the description is
    /// new.
    #[test]
    fn the_library_carries_how_its_modules_were_built() {
        let stdlib = Stdlib::build();

        match stdlib.structure_of(stdlib.adder24) {
            Some(Structure::Chain(lane)) => {
                assert_eq!(lane.label, "sum");
                assert_eq!(
                    lane.body,
                    CellKind::Instance {
                        module: stdlib.full_adder
                    }
                );
                assert_eq!(lane.cells.len(), 24, "one full adder per bit");
            }
            other => panic!("the 24-bit adder is a chain, got {other:?}"),
        }

        let select16 = stdlib
            .library
            .modules()
            .find(|m| m.name == "std.select16")
            .expect("the selector")
            .id;
        match stdlib.structure_of(select16) {
            Some(Structure::Fold(term)) => {
                assert_eq!(term.label, "select");
                assert_eq!(term.body, CellKind::Instance { module: stdlib.mux });
                assert_eq!(
                    term.levels.iter().map(Vec::len).collect::<Vec<_>>(),
                    vec![8, 4, 2, 1]
                );
            }
            other => panic!("the selector is a fold, got {other:?}"),
        }

        // A module nobody has described is not different, only
        // undescribed — the absence must read as "no term", never as an
        // error or a wrong one.
        //
        // The two examples are leaf gates, which have nothing to repeat
        // and so will stay undescribed. A composite module that later
        // acquires retained structure would make the test fail for a
        // success; this checks leaf gates rather than description coverage.
        assert!(stdlib.structure_of(stdlib.mux).is_none());
        assert!(stdlib.structure_of(stdlib.not_gate).is_none());
    }

    /// Whatever the shape, a term accounts for cells that exist. A viewer
    /// folding boxes that are not there would be worse than one that does
    /// not fold.
    #[test]
    fn every_retained_term_names_cells_of_its_own_module() {
        let stdlib = Stdlib::build();
        for module in stdlib.library.modules() {
            let Some(term) = stdlib.structure_of(module.id) else {
                continue;
            };
            let present: Vec<_> = module.cells.iter().map(|c| c.id).collect();
            let named = term.cells();
            assert!(!named.is_empty(), "{} has an empty term", module.name);
            assert!(
                named.iter().all(|cell| present.contains(cell)),
                "{} names a cell it does not have",
                module.name
            );
        }
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use crate::model::ModuleLibrary;
    use crate::stdlib::{Stdlib, build_ripple_adder_scanned};

    /// **A term travels with its module.**
    ///
    /// It used to live in a private table beside one library, which meant
    /// it was not retained at all: it could not move with the module, no
    /// builder outside that library could attach one, and anything that
    /// rebuilt a library lost it. A review called that out and it was
    /// right — "retained" was a claim the code did not keep.
    #[test]
    fn a_term_belongs_to_the_module_not_to_a_library() {
        let stdlib = Stdlib::build();
        let (module, _) =
            build_ripple_adder_scanned("t.owned", 8, stdlib.full_adder, &stdlib.library);

        // Built by a caller that is not `Stdlib::build`, and described
        // anyway.
        assert_eq!(module.structures.len(), 1, "the builder recorded it");

        // Put into a library nobody special owns, and it is still there.
        let id = module.id;
        let mut library = ModuleLibrary::new();
        library.insert(module);
        let back = library.get(id).expect("in the library");
        match back.structures.first() {
            Some(Structure::Chain(lane)) => assert_eq!(lane.cells.len(), 8),
            other => panic!("the term did not travel: {other:?}"),
        }

        // And a clone of the library carries it, which a side table
        // keyed by one `Stdlib` could not.
        let copy = library.clone();
        assert_eq!(
            copy.get(id).expect("cloned").structures,
            back.structures,
            "a term survives copying the library"
        );
    }

    /// Recording is not optional and not a second step: `scan` and `tree`
    /// put the term on the module themselves, so a builder cannot state a
    /// repetition and forget to keep it.
    #[test]
    fn stating_a_repetition_keeps_it() {
        let stdlib = Stdlib::build();
        for (name, width) in [("t.a", 4), ("t.b", 16)] {
            let (module, lane) =
                build_ripple_adder_scanned(name, width, stdlib.full_adder, &stdlib.library);
            assert_eq!(
                module.structures,
                vec![Structure::Chain(lane)],
                "what was returned is what was kept"
            );
        }
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::crdf_io::{asset_from_rdf, asset_to_rdf};
    use crate::model::CircuitAsset;
    use crate::stdlib::{Stdlib, build_ripple_adder_scanned, build_select16_treed};
    // The map case reuses the bank the equivalence tests build, so both
    // sides are asking about the same term.
    use super::tests::{WIDTH, bitwise_bank_mapped};

    /// Through the whole file format an asset actually uses, rather than
    /// a module-only shortcut: what matters is that a saved *circuit*
    /// keeps its provenance.
    fn round_trip(module: &crate::model::Module, stdlib: &Stdlib) -> crate::model::Module {
        let id = module.id;
        let mut library = stdlib.library.clone();
        library.insert(module.clone());
        let graph = asset_to_rdf(&CircuitAsset::new(id, 1, library)).expect("writes");
        asset_from_rdf(&graph)
            .expect("reads")
            .library
            .get(id)
            .expect("the module")
            .clone()
    }

    /// **A term survives a save**, which is the difference between a
    /// description and a demonstration.
    ///
    /// Until this, provenance lived only in memory: everything that
    /// reloaded a circuit lost it, so a viewer could fold a freshly built
    /// library and never a saved one.
    #[test]
    fn a_chain_and_a_fold_both_survive_the_file_format() {
        let stdlib = Stdlib::build();

        let (adder, lane) =
            build_ripple_adder_scanned("t.persist.chain", 8, stdlib.full_adder, &stdlib.library);
        match round_trip(&adder, &stdlib).structures.as_slice() {
            [Structure::Chain(back)] => assert_eq!(*back, lane, "the chain came back whole"),
            other => panic!("expected one chain, got {other:?}"),
        }

        let (selector, tree) = build_select16_treed("t.persist.fold", stdlib.mux, &stdlib.library);
        match round_trip(&selector, &stdlib).structures.as_slice() {
            [Structure::Fold(back)] => {
                assert_eq!(*back, tree, "the fold came back whole");
                assert_eq!(
                    back.levels.iter().map(Vec::len).collect::<Vec<_>>(),
                    vec![8, 4, 2, 1],
                    "including its levels, which a flat list would have lost"
                );
            }
            other => panic!("expected one fold, got {other:?}"),
        }
    }

    /// **A map survives a save as a map**, not as a chain.
    ///
    /// The two carry identical payloads, so a serialiser that wrote only
    /// the cells would round-trip without complaint and hand back a term
    /// asserting a dependency between lanes that does not exist. The
    /// kind is what distinguishes them and the kind is what this checks.
    #[test]
    fn a_map_survives_the_file_format_as_a_map() {
        let stdlib = Stdlib::build();
        let bank = bitwise_bank_mapped("t.persist.map", WIDTH, &stdlib);
        let Structure::Map(built) = bank.structures[0].clone() else {
            panic!("a bank of independent gates is a map");
        };

        match round_trip(&bank, &stdlib).structures.as_slice() {
            [Structure::Map(back)] => assert_eq!(*back, built, "the map came back whole"),
            other => panic!("expected one map, got {other:?}"),
        }
    }

    /// **Every term in the library survives a round trip.**
    #[test]
    fn every_stdlib_term_round_trips() {
        let stdlib = Stdlib::build();
        for module in stdlib.library.modules() {
            if module.structures.is_empty() {
                continue;
            }
            let back = round_trip(module, &stdlib);
            assert_eq!(
                back.structures.len(),
                module.structures.len(),
                "{} lost or gained terms",
                module.name
            );
            for (before, after) in module.structures.iter().zip(&back.structures) {
                assert_eq!(before, after, "{}: a term came back different", module.name);
            }
        }
    }

    /// A module nobody described comes back **undescribed**, not wrongly
    /// described. Reading provenance that is not there must never invent
    /// any.
    #[test]
    fn a_module_without_a_term_gains_none() {
        let stdlib = Stdlib::build();
        let mux = stdlib.library.get(stdlib.mux).expect("the mux").clone();
        assert!(mux.structures.is_empty(), "nobody described the mux");
        assert!(
            round_trip(&mux, &stdlib).structures.is_empty(),
            "and a round trip may not invent a term"
        );
    }
}
