//! Standard library modules, built from `nand` + `reg` only.
//!
//! These are original constructions of textbook logic: the elementary
//! gates (NOT / AND / OR / NOR / XOR / XNOR / MUX), half and full
//! adders (the classic 9-NAND form), ripple-carry addition, a 16×16
//! multiplier and a Fibonacci LFSR — generic building blocks with no
//! domain semantics. Consumers compose them into their own top-level circuits.
//!
//! Module port names use the `<bus>.<bit>` convention with LSB 0.
//! Port names carry no semantics here; any binding convention is the
//! consumer's concern.

use crate::model::{CellKind, Endpoint, Module, ModuleBuilder, ModuleId, ModuleLibrary, PortId};
use crate::structure::{self, Lane, Structure, Tree, scan, tree};

pub const LFSR_BITS: usize = 16;

/// The standard library: a module set plus the ids of its entry points.
#[derive(Debug, Clone)]
pub struct Stdlib {
    pub library: ModuleLibrary,
    /// `a` → `y` (1 NAND).
    pub not_gate: ModuleId,
    /// `a`, `b` → `y` (2 NANDs).
    pub and_gate: ModuleId,
    /// `a`, `b` → `y` (3 NANDs).
    pub or_gate: ModuleId,
    /// `a`, `b` → `y` (4 NANDs).
    pub nor_gate: ModuleId,
    /// `a`, `b` → `y` (4 NANDs).
    pub xor_gate: ModuleId,
    /// `a`, `b` → `y` (5 NANDs).
    pub xnor_gate: ModuleId,
    /// `sel`, `a`, `b` → `y = sel ? b : a` (4 NANDs).
    pub mux: ModuleId,
    /// `a`, `b` → `sum`, `carry` (xor + and instances).
    pub half_adder: ModuleId,
    /// `a`, `b`, `cin` → `sum`, `cout` (9 NANDs).
    pub full_adder: ModuleId,
    /// `a.<0..23>`, `b.<0..23>`, `cin` → `sum.<0..23>`, `cout`.
    pub adder24: ModuleId,
    /// `a.<0..15>`, `b.<0..15>` → `p.<0..31>`: unsigned array multiplier.
    pub mul16: ModuleId,
    /// A 24x24 unsigned multiply -- the mantissa product a binary32
    /// multiply is built around.
    pub umul24: ModuleId,
    /// A binary32 multiply: flush-to-zero, saturating, round to nearest.
    /// Not IEEE-754; see `build_fmul32` for exactly what it is.
    pub fmul32: ModuleId,
    /// `out`: free-running maximal-length 16-bit Fibonacci LFSR.
    pub lfsr16: ModuleId,
}

impl Stdlib {
    pub fn build() -> Self {
        let mut library = ModuleLibrary::new();

        let not_gate_id = insert(&mut library, build_not());
        let and_gate_id = insert(&mut library, build_and());
        let or_gate_id = insert(&mut library, build_or());
        let nor_gate_id = insert(&mut library, build_nor());
        let xor_gate_id = insert(&mut library, build_xor_module());
        let xnor_gate_id = insert(&mut library, build_xnor());
        let mux_id = insert(&mut library, build_mux());
        let half_adder = build_half_adder(xor_gate_id, and_gate_id, &library);
        let half_adder_id = insert(&mut library, half_adder);

        let full_adder = build_full_adder();
        let full_adder_id = full_adder.id;
        library.insert(full_adder);

        let (adder24, _) = build_ripple_adder_scanned("std.adder24", 24, full_adder_id, &library);
        let adder24_id = adder24.id;
        library.insert(adder24);

        let (mul16, mul16_row) =
            build_multiplier("std.mul16", 16, full_adder_id, and_gate_id, &library);
        insert(&mut library, mul16_row);
        let mul16_id = insert(&mut library, mul16);

        // A 24x24 unsigned multiply, which is what a binary32 mantissa
        // product needs. The same builder as `std.mul16`, one width up.
        let (umul24, umul24_row) =
            build_multiplier("std.umul24", 24, full_adder_id, and_gate_id, &library);
        insert(&mut library, umul24_row);
        let umul24_id = insert(&mut library, umul24);
        let fmul32 = build_fmul32(
            and_gate_id,
            or_gate_id,
            xor_gate_id,
            not_gate_id,
            full_adder_id,
            umul24_id,
            &library,
        );
        let fmul32_id = insert(&mut library, fmul32);

        let (adder16, _) = build_ripple_adder_scanned("std.adder16", 16, full_adder_id, &library);
        let adder16_id = adder16.id;
        library.insert(adder16);
        let ge16 = build_ge16(adder16_id, not_gate_id, &library);
        library.insert(ge16);

        let (select16, _) = build_select16_treed("std.select16", mux_id, &library);
        library.insert(select16);

        let lfsr16 = build_lfsr16(xor_gate_id, &library);
        let lfsr16_id = lfsr16.id;
        library.insert(lfsr16);

        Self {
            library,
            not_gate: not_gate_id,
            and_gate: and_gate_id,
            or_gate: or_gate_id,
            nor_gate: nor_gate_id,
            xor_gate: xor_gate_id,
            xnor_gate: xnor_gate_id,
            mux: mux_id,
            half_adder: half_adder_id,
            full_adder: full_adder_id,
            adder24: adder24_id,
            mul16: mul16_id,
            umul24: umul24_id,
            fmul32: fmul32_id,
            lfsr16: lfsr16_id,
        }
    }

    /// How `module` was built, if it was built out of a retained term.
    ///
    /// A viewer holding this draws `FULL ADDER ×24` because it was told,
    /// not because it guessed — which is the difference between
    /// certainty and the eleven per cent a heuristic managed.
    pub fn structure_of(&self, module: ModuleId) -> Option<&Structure> {
        self.library.get(module)?.structures.first()
    }
}

fn insert(library: &mut ModuleLibrary, module: Module) -> ModuleId {
    let id = module.id;
    library.insert(module);
    id
}

/// `y = !a` (1 NAND).
fn build_not() -> Module {
    let mut m = ModuleBuilder::new_canonical("std.not");
    let a = m.input("a");
    let y = m.output("y");
    let gate = m.not_gate(Endpoint::module(a));
    m.wire(Endpoint::nand_y(gate), Endpoint::module(y));
    m.finish()
}

/// `y = a & b` (2 NANDs).
fn build_and() -> Module {
    let mut m = ModuleBuilder::new_canonical("std.and");
    let a = m.input("a");
    let b = m.input("b");
    let y = m.output("y");
    let nand = m.nand();
    m.wire(Endpoint::module(a), Endpoint::nand_a(nand));
    m.wire(Endpoint::module(b), Endpoint::nand_b(nand));
    let inv = m.not_gate(Endpoint::nand_y(nand));
    m.wire(Endpoint::nand_y(inv), Endpoint::module(y));
    m.finish()
}

/// `y = a | b` (3 NANDs, De Morgan).
fn build_or() -> Module {
    let mut m = ModuleBuilder::new_canonical("std.or");
    let a = m.input("a");
    let b = m.input("b");
    let y = m.output("y");
    let not_a = m.not_gate(Endpoint::module(a));
    let not_b = m.not_gate(Endpoint::module(b));
    let nand = m.nand();
    m.wire(Endpoint::nand_y(not_a), Endpoint::nand_a(nand));
    m.wire(Endpoint::nand_y(not_b), Endpoint::nand_b(nand));
    m.wire(Endpoint::nand_y(nand), Endpoint::module(y));
    m.finish()
}

/// `y = !(a | b)` (4 NANDs).
fn build_nor() -> Module {
    let mut m = ModuleBuilder::new_canonical("std.nor");
    let a = m.input("a");
    let b = m.input("b");
    let y = m.output("y");
    let not_a = m.not_gate(Endpoint::module(a));
    let not_b = m.not_gate(Endpoint::module(b));
    let nand = m.nand();
    m.wire(Endpoint::nand_y(not_a), Endpoint::nand_a(nand));
    m.wire(Endpoint::nand_y(not_b), Endpoint::nand_b(nand));
    let inv = m.not_gate(Endpoint::nand_y(nand));
    m.wire(Endpoint::nand_y(inv), Endpoint::module(y));
    m.finish()
}

/// `y = a ^ b` (the classic 4-NAND XOR).
fn build_xor_module() -> Module {
    let mut m = ModuleBuilder::new_canonical("std.xor");
    let a = m.input("a");
    let b = m.input("b");
    let y = m.output("y");
    let out = build_xor(&mut m, Endpoint::module(a), Endpoint::module(b));
    m.wire(out, Endpoint::module(y));
    m.finish()
}

/// `y = !(a ^ b)` (5 NANDs).
fn build_xnor() -> Module {
    let mut m = ModuleBuilder::new_canonical("std.xnor");
    let a = m.input("a");
    let b = m.input("b");
    let y = m.output("y");
    let out = build_xor(&mut m, Endpoint::module(a), Endpoint::module(b));
    let inv = m.not_gate(out);
    m.wire(Endpoint::nand_y(inv), Endpoint::module(y));
    m.finish()
}

/// `y = sel ? b : a` (4 NANDs).
fn build_mux() -> Module {
    let mut m = ModuleBuilder::new_canonical("std.mux");
    let sel = m.input("sel");
    let a = m.input("a");
    let b = m.input("b");
    let y = m.output("y");
    let not_sel = m.not_gate(Endpoint::module(sel));
    let pick_a = m.nand();
    m.wire(Endpoint::module(a), Endpoint::nand_a(pick_a));
    m.wire(Endpoint::nand_y(not_sel), Endpoint::nand_b(pick_a));
    let pick_b = m.nand();
    m.wire(Endpoint::module(b), Endpoint::nand_a(pick_b));
    m.wire(Endpoint::module(sel), Endpoint::nand_b(pick_b));
    let combine = m.nand();
    m.wire(Endpoint::nand_y(pick_a), Endpoint::nand_a(combine));
    m.wire(Endpoint::nand_y(pick_b), Endpoint::nand_b(combine));
    m.wire(Endpoint::nand_y(combine), Endpoint::module(y));
    m.finish()
}

/// `sum = a ^ b`, `carry = a & b` — built from gate instances to keep
/// the hierarchy path exercised.
fn build_half_adder(xor_gate: ModuleId, and_gate: ModuleId, library: &ModuleLibrary) -> Module {
    let mut m = ModuleBuilder::new_canonical("std.half_adder");
    let a = m.input("a");
    let b = m.input("b");
    let sum = m.output("sum");
    let carry = m.output("carry");

    let xor = m.instance(xor_gate);
    m.wire(
        Endpoint::module(a),
        Endpoint::inner(xor, port_id(library, xor_gate, "a")),
    );
    m.wire(
        Endpoint::module(b),
        Endpoint::inner(xor, port_id(library, xor_gate, "b")),
    );
    m.wire(
        Endpoint::inner(xor, port_id(library, xor_gate, "y")),
        Endpoint::module(sum),
    );

    let and = m.instance(and_gate);
    m.wire(
        Endpoint::module(a),
        Endpoint::inner(and, port_id(library, and_gate, "a")),
    );
    m.wire(
        Endpoint::module(b),
        Endpoint::inner(and, port_id(library, and_gate, "b")),
    );
    m.wire(
        Endpoint::inner(and, port_id(library, and_gate, "y")),
        Endpoint::module(carry),
    );
    m.finish()
}

/// The classic 9-NAND full adder.
fn build_full_adder() -> Module {
    let mut m = ModuleBuilder::new_canonical("std.full_adder");
    let a = m.input("a");
    let b = m.input("b");
    let cin = m.input("cin");
    let sum = m.output("sum");
    let cout = m.output("cout");

    let n1 = m.nand();
    m.wire(Endpoint::module(a), Endpoint::nand_a(n1));
    m.wire(Endpoint::module(b), Endpoint::nand_b(n1));
    let n2 = m.nand();
    m.wire(Endpoint::module(a), Endpoint::nand_a(n2));
    m.wire(Endpoint::nand_y(n1), Endpoint::nand_b(n2));
    let n3 = m.nand();
    m.wire(Endpoint::module(b), Endpoint::nand_a(n3));
    m.wire(Endpoint::nand_y(n1), Endpoint::nand_b(n3));
    // n4 = a xor b
    let n4 = m.nand();
    m.wire(Endpoint::nand_y(n2), Endpoint::nand_a(n4));
    m.wire(Endpoint::nand_y(n3), Endpoint::nand_b(n4));
    let n5 = m.nand();
    m.wire(Endpoint::nand_y(n4), Endpoint::nand_a(n5));
    m.wire(Endpoint::module(cin), Endpoint::nand_b(n5));
    let n6 = m.nand();
    m.wire(Endpoint::nand_y(n4), Endpoint::nand_a(n6));
    m.wire(Endpoint::nand_y(n5), Endpoint::nand_b(n6));
    let n7 = m.nand();
    m.wire(Endpoint::module(cin), Endpoint::nand_a(n7));
    m.wire(Endpoint::nand_y(n5), Endpoint::nand_b(n7));
    let n8 = m.nand();
    m.wire(Endpoint::nand_y(n6), Endpoint::nand_a(n8));
    m.wire(Endpoint::nand_y(n7), Endpoint::nand_b(n8));
    m.wire(Endpoint::nand_y(n8), Endpoint::module(sum));
    let n9 = m.nand();
    m.wire(Endpoint::nand_y(n5), Endpoint::nand_a(n9));
    m.wire(Endpoint::nand_y(n1), Endpoint::nand_b(n9));
    m.wire(Endpoint::nand_y(n9), Endpoint::module(cout));

    m.finish()
}

/// A ripple-carry adder of any width, for callers outside this module.
///
/// The adder itself was never missing — only a way to reach it. Ports: `a.{i}`, `b.{i}`, `cin` -> `sum.{i}`, `cout`.
///
/// The carry chain is a **scan**, and `build_ripple_adder_scanned` says
/// so with a retained term (`crate::structure`). This one stays as the
/// control the golden spike is measured against.
pub fn build_ripple_adder(
    name: &str,
    width: usize,
    full_adder: ModuleId,
    library: &ModuleLibrary,
) -> Module {
    let fa_ports = adder_port_ids(library, full_adder);

    let mut m = ModuleBuilder::new_canonical(name);
    let a: Vec<PortId> = (0..width).map(|i| m.input(format!("a.{i}"))).collect();
    let b: Vec<PortId> = (0..width).map(|i| m.input(format!("b.{i}"))).collect();
    let cin = m.input("cin");
    let sum: Vec<PortId> = (0..width).map(|i| m.output(format!("sum.{i}"))).collect();
    let cout = m.output("cout");

    let mut carry: Endpoint = Endpoint::module(cin);
    for bit in 0..width {
        let fa = m.instance(full_adder);
        m.wire(Endpoint::module(a[bit]), Endpoint::inner(fa, fa_ports.a));
        m.wire(Endpoint::module(b[bit]), Endpoint::inner(fa, fa_ports.b));
        m.wire(carry, Endpoint::inner(fa, fa_ports.cin));
        m.wire(
            Endpoint::inner(fa, fa_ports.sum),
            Endpoint::module(sum[bit]),
        );
        carry = Endpoint::inner(fa, fa_ports.cout);
    }
    m.wire(carry, Endpoint::module(cout));

    m.finish()
}

/// The same adder, said as a **scan** and remembered as one.
///
/// The first retained structure. It emits the
/// same cells and the same wires as [`build_ripple_adder`] — the tests
/// `a_scanned_adder_is_the_same_circuit` and `..._computes_the_same_sums`
/// hold that — but it also returns a [`Lane`], so what the author knew
/// ("these twenty-four full adders are one carry chain") survives instead
/// of having to be guessed at afterwards by whoever draws it.
///
/// That is the whole claim: **declarative construction emits today's
/// artefact, and the repetition is retained.**
pub fn build_ripple_adder_scanned(
    name: &str,
    width: usize,
    full_adder: ModuleId,
    library: &ModuleLibrary,
) -> (Module, Lane) {
    let fa = adder_port_ids(library, full_adder);

    let mut m = ModuleBuilder::new_canonical(name);
    let a: Vec<PortId> = (0..width).map(|i| m.input(format!("a.{i}"))).collect();
    let b: Vec<PortId> = (0..width).map(|i| m.input(format!("b.{i}"))).collect();
    let cin = m.input("cin");
    let sum: Vec<PortId> = (0..width).map(|i| m.output(format!("sum.{i}"))).collect();
    let cout = m.output("cout");

    let (lane, carry) = scan(
        &mut m,
        "sum",
        width,
        CellKind::Instance { module: full_adder },
        Endpoint::module(cin),
        |m, bit, cell, carry_in| {
            m.wire(Endpoint::module(a[bit]), Endpoint::inner(cell, fa.a));
            m.wire(Endpoint::module(b[bit]), Endpoint::inner(cell, fa.b));
            m.wire(carry_in, Endpoint::inner(cell, fa.cin));
            m.wire(Endpoint::inner(cell, fa.sum), Endpoint::module(sum[bit]));
        },
        |cell| Endpoint::inner(cell, fa.cout),
    );
    m.wire(carry, Endpoint::module(cout));

    (m.finish(), lane)
}

/// Port ids of a full adder, for composing instances.
pub struct AdderPorts {
    pub a: PortId,
    pub b: PortId,
    pub cin: PortId,
    pub sum: PortId,
    pub cout: PortId,
}

/// Looks up the `a`/`b`/`cin`/`sum`/`cout` ports of a full adder.
pub fn adder_port_ids(library: &ModuleLibrary, full_adder: ModuleId) -> AdderPorts {
    let module = library.get(full_adder).expect("full adder in library");
    let find = |name: &str| {
        module
            .ports
            .iter()
            .find(|p| p.name == name)
            .expect("full adder port")
            .id
    };
    AdderPorts {
        a: find("a"),
        b: find("b"),
        cin: find("cin"),
        sum: find("sum"),
        cout: find("cout"),
    }
}

/// Looks up a named port of a library module, or `None` when the module
/// is not in the library or has no port under that name.
///
/// Use this wherever the name could have come from outside this crate —
/// a saved interface, a hand-edited file, a module built elsewhere.
/// [`port_id`] is the shorthand for the other case, where the module was
/// just built here and the name is a literal a few lines above.
pub fn try_port_id(library: &ModuleLibrary, module: ModuleId, name: &str) -> Option<PortId> {
    library
        .get(module)?
        .ports
        .iter()
        .find(|p| p.name == name)
        .map(|p| p.id)
}

/// Looks up a named port of a library module, **panicking when absent**.
///
/// Every caller here is assembling a module this crate has just built,
/// naming a port it wrote a few lines earlier, so absence is a bug in
/// this file rather than bad input. Anything holding a name that came
/// from a file must use [`try_port_id`] instead: a mismatched interface
/// is a thing to report, not a thing to crash on.
pub fn port_id(library: &ModuleLibrary, module: ModuleId, name: &str) -> PortId {
    try_port_id(library, module, name).expect("a port this crate just built")
}

/// A gate-instance helper bundle for composite builders.
pub struct GatePorts {
    pub a: PortId,
    pub b: PortId,
    pub y: PortId,
}

/// Looks up the `a`/`b`/`y` ports of a 2-input gate module.
pub fn gate_ports(library: &ModuleLibrary, module: ModuleId) -> GatePorts {
    GatePorts {
        a: port_id(library, module, "a"),
        b: port_id(library, module, "b"),
        y: port_id(library, module, "y"),
    }
}

/// `width`×`width` unsigned array multiplier from AND rows and ripple
/// full-adder chains: `a.<i>`, `b.<i>` → `p.<0..2·width-1>`.
/// One row of the shift-and-add multiplier: the partial products of `a`
/// with a single bit of `b`, added into the running sum.
///
/// `a.{j}` × `b`, plus `sin.{j}` and `cin` → `sout.{j}` and `cout`.
///
/// Extracted purely so the multiplier can be **read**. A level of
/// abstraction is not a level of circuit — flattening removes this
/// boundary and the gate and register counts do not move — but without
/// it `std.mul16`
/// presents 497 cells at one level, which no schematic viewer and no
/// person can take in. With it the multiplier is sixteen boxes and a
/// row is thirty-two, of which sixteen are one kind and sixteen the
/// other.
fn build_mul_row(
    name: &str,
    width: usize,
    full_adder: ModuleId,
    and_gate: ModuleId,
    library: &ModuleLibrary,
) -> Module {
    let fa = adder_port_ids(library, full_adder);
    let and = gate_ports(library, and_gate);

    let mut m = ModuleBuilder::new_canonical(name);
    let a: Vec<PortId> = (0..width).map(|j| m.input(format!("a.{j}"))).collect();
    let b = m.input("b");
    let sin: Vec<PortId> = (0..width).map(|j| m.input(format!("sin.{j}"))).collect();
    let cin = m.input("cin");
    let sout: Vec<PortId> = (0..width).map(|j| m.output(format!("sout.{j}"))).collect();
    let cout = m.output("cout");

    // An AND and an adder per bit, turn and turn about, so the loop
    // stays as it is and two terms are recorded over it: the partial
    // products are independent of each other, the adders thread a carry.
    // Emission order is identity here — a cell's canonical id is a hash
    // of its ordinal — so the one thing that must not happen is
    // separating the two into two loops to make them look tidy.
    let mut partials = Vec::with_capacity(width);
    let mut adders = Vec::with_capacity(width);
    let mut carry = Endpoint::module(cin);
    for j in 0..width {
        let partial = m.instance(and_gate);
        m.wire(Endpoint::module(a[j]), Endpoint::inner(partial, and.a));
        m.wire(Endpoint::module(b), Endpoint::inner(partial, and.b));
        partials.push(partial);

        let add = m.instance(full_adder);
        m.wire(Endpoint::inner(partial, and.y), Endpoint::inner(add, fa.a));
        m.wire(Endpoint::module(sin[j]), Endpoint::inner(add, fa.b));
        m.wire(carry, Endpoint::inner(add, fa.cin));
        m.wire(Endpoint::inner(add, fa.sum), Endpoint::module(sout[j]));
        carry = Endpoint::inner(add, fa.cout);
        adders.push(add);
    }
    structure::record_map(
        &mut m,
        "partial",
        CellKind::Instance { module: and_gate },
        partials,
    );
    structure::record_chain(
        &mut m,
        "accumulate",
        CellKind::Instance { module: full_adder },
        adders,
    );
    m.wire(carry, Endpoint::module(cout));
    m.finish()
}

/// Returns `(the multiplier, its row definition)`; the caller inserts
/// both into the library.
fn build_multiplier(
    name: &str,
    width: usize,
    full_adder: ModuleId,
    and_gate: ModuleId,
    library: &ModuleLibrary,
) -> (Module, Module) {
    // Only the first row of partial products is built here; the rest are
    // instances of the row module, so this needs the AND ports and not
    // the adder's.
    let and = gate_ports(library, and_gate);

    let mut m = ModuleBuilder::new_canonical(name);
    let a: Vec<PortId> = (0..width).map(|i| m.input(format!("a.{i}"))).collect();
    let b: Vec<PortId> = (0..width).map(|i| m.input(format!("b.{i}"))).collect();
    let p: Vec<PortId> = (0..2 * width).map(|i| m.output(format!("p.{i}"))).collect();

    let zero = m.constant(false);
    let zero_ep = Endpoint::reg_q(zero);

    // Running sum, one endpoint per product bit.
    //
    // The first row of partial products: sixteen ANDs of `a` against
    // `b.0`, independent of one another, so one map. (This was a closure
    // returning the output endpoint; it is written out because a term
    // has to name the *cells*, and the endpoint was all the closure gave
    // back. The instances are emitted in the same order either way.)
    let mut sum: Vec<Option<Endpoint>> = vec![None; 2 * width];
    let mut partials = Vec::with_capacity(width);
    for (j, a_bit) in a.iter().enumerate() {
        let inst = m.instance(and_gate);
        m.wire(Endpoint::module(*a_bit), Endpoint::inner(inst, and.a));
        m.wire(Endpoint::module(b[0]), Endpoint::inner(inst, and.b));
        sum[j] = Some(Endpoint::inner(inst, and.y));
        partials.push(inst);
    }
    structure::record_map(
        &mut m,
        "partial",
        CellKind::Instance { module: and_gate },
        partials,
    );
    // One instance per row of partial products, rather than thirty-two
    // loose cells per row written into this module. Same circuit; a
    // schematic somebody can follow.
    let row = build_mul_row(&format!("{name}.row"), width, full_adder, and_gate, library);
    let row_id = row.id;
    let port_of = |name: &str| {
        row.ports
            .iter()
            .find(|port| port.name == name)
            .map(|port| port.id)
            .expect("the row declares the port it was just built with")
    };
    let row_a: Vec<PortId> = (0..width).map(|j| port_of(&format!("a.{j}"))).collect();
    let row_sin: Vec<PortId> = (0..width).map(|j| port_of(&format!("sin.{j}"))).collect();
    let row_sout: Vec<PortId> = (0..width).map(|j| port_of(&format!("sout.{j}"))).collect();
    let (row_b, row_cin, row_cout) = (port_of("b"), port_of("cin"), port_of("cout"));

    // Each row takes the running sum the row before it produced, so
    // these are a chain and not a map, however alike they look.
    let mut rows = Vec::with_capacity(width.saturating_sub(1));
    for i in 1..width {
        let inst = m.instance(row_id);
        for j in 0..width {
            m.wire(Endpoint::module(a[j]), Endpoint::inner(inst, row_a[j]));
            m.wire(
                sum[i + j].unwrap_or(zero_ep),
                Endpoint::inner(inst, row_sin[j]),
            );
        }
        m.wire(Endpoint::module(b[i]), Endpoint::inner(inst, row_b));
        m.wire(zero_ep, Endpoint::inner(inst, row_cin));
        for j in 0..width {
            sum[i + j] = Some(Endpoint::inner(inst, row_sout[j]));
        }
        sum[i + width] = Some(Endpoint::inner(inst, row_cout));
        rows.push(inst);
    }
    structure::record_chain(&mut m, "rows", CellKind::Instance { module: row_id }, rows);
    for (bit, port) in p.iter().enumerate() {
        m.wire(sum[bit].unwrap_or(zero_ep), Endpoint::module(*port));
    }
    (m.finish(), row)
}

/// One of sixteen one-bit values, chosen by a four-bit index.
///
/// A balanced tree of fifteen `std.mux`. A state machine that selects
/// several of its sixteen-way values this way, many times over, inlines
/// to hundreds of muxes in one module and becomes unreadable; as a
/// module, each selection is one box.
///
/// Ports: `v.0..15`, `sel.0..3` -> `y`.
/// The same selector, said as a **tree** and remembered as one.
///
/// The second retained structure, after the scanned adder. Its loop already looked combinatorial — halve the
/// layer, pair the neighbours, repeat — which is exactly why it is the
/// right second test: if saying `tree` changes nothing about the circuit
/// and everything about what is known afterwards, then what was missing
/// was **retention**, not a nicer way to write the loop.
pub fn build_select16_treed(name: &str, mux: ModuleId, library: &ModuleLibrary) -> (Module, Tree) {
    let mux_sel = port_id(library, mux, "sel");
    let mux_a = port_id(library, mux, "a");
    let mux_b = port_id(library, mux, "b");
    let mux_y = port_id(library, mux, "y");

    let mut m = ModuleBuilder::new_canonical(name);
    let leaves: Vec<Endpoint> = (0..16)
        .map(|i| Endpoint::module(m.input(format!("v.{i}"))))
        .collect();
    let sel: Vec<Endpoint> = (0..4)
        .map(|bit| Endpoint::module(m.input(format!("sel.{bit}"))))
        .collect();
    let y = m.output("y");

    // Level `k` of the fold is selected by bit `k`: the tree's depth and
    // the index's width are the same number, which is what makes this a
    // fold rather than a chain.
    let (term, chosen) = tree(
        &mut m,
        "select",
        &leaves,
        CellKind::Instance { module: mux },
        |m, level, cell, a, b| {
            m.wire(sel[level], Endpoint::inner(cell, mux_sel));
            m.wire(a, Endpoint::inner(cell, mux_a));
            m.wire(b, Endpoint::inner(cell, mux_b));
        },
        |cell| Endpoint::inner(cell, mux_y),
    );
    m.wire(chosen, Endpoint::module(y));
    (m.finish(), term)
}

/// The loop-written selector, kept as the control the treed one is
/// measured against — the library itself uses `build_select16_treed`.
pub fn build_select16_plain(name: &str, mux: ModuleId, library: &ModuleLibrary) -> Module {
    let mux_sel = port_id(library, mux, "sel");
    let mux_a = port_id(library, mux, "a");
    let mux_b = port_id(library, mux, "b");
    let mux_y = port_id(library, mux, "y");

    let mut m = ModuleBuilder::new_canonical(name);
    let mut layer: Vec<Endpoint> = (0..16)
        .map(|i| Endpoint::module(m.input(format!("v.{i}"))))
        .collect();
    let sel: Vec<Endpoint> = (0..4)
        .map(|bit| Endpoint::module(m.input(format!("sel.{bit}"))))
        .collect();
    let y = m.output("y");

    for bit in sel {
        layer = layer
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                let cell = m.instance(mux);
                m.wire(bit, Endpoint::inner(cell, mux_sel));
                m.wire(pair[0], Endpoint::inner(cell, mux_a));
                m.wire(pair[1], Endpoint::inner(cell, mux_b));
                Endpoint::inner(cell, mux_y)
            })
            .collect();
    }
    m.wire(layer[0], Endpoint::module(y));
    m.finish()
}

/// `a >= b` on 16-bit unsigned words, as a module.
///
/// The comparison is a subtraction whose sum is discarded: `a + !b + 1`
/// carries out exactly when `a >= b`. Extracted so a caller draws one
/// box where it had sixteen inverters and sixteen full adders. `one` is
/// lent by the parent,
/// because a minted constant is a register.
///
/// Ports: `a.0..15`, `b.0..15`, `one` -> `ge`.
fn build_ge16(adder16: ModuleId, not_gate: ModuleId, library: &ModuleLibrary) -> Module {
    let not_a = port_id(library, not_gate, "a");
    let not_y = port_id(library, not_gate, "y");

    let mut m = ModuleBuilder::new_canonical("std.ge16");
    let a: Vec<Endpoint> = (0..16)
        .map(|i| Endpoint::module(m.input(format!("a.{i}"))))
        .collect();
    let b: Vec<Endpoint> = (0..16)
        .map(|i| Endpoint::module(m.input(format!("b.{i}"))))
        .collect();
    let one = Endpoint::module(m.input("one"));
    let ge = m.output("ge");

    // One inverter per bit of `b`, for the two's-complement subtraction
    // whose carry-out is the comparison. Independent of each other --
    // the carry they feed is the adder's business, not theirs.
    let add = m.instance(adder16);
    let mut invert = Vec::with_capacity(16);
    for i in 0..16 {
        let inv = m.instance(not_gate);
        invert.push(inv);
        m.wire(b[i], Endpoint::inner(inv, not_a));
        m.wire(
            a[i],
            Endpoint::inner(add, port_id(library, adder16, &format!("a.{i}"))),
        );
        m.wire(
            Endpoint::inner(inv, not_y),
            Endpoint::inner(add, port_id(library, adder16, &format!("b.{i}"))),
        );
    }
    structure::record_map(
        &mut m,
        "invert b",
        CellKind::Instance { module: not_gate },
        invert,
    );
    m.wire(one, Endpoint::inner(add, port_id(library, adder16, "cin")));
    m.wire(
        Endpoint::inner(add, port_id(library, adder16, "cout")),
        Endpoint::module(ge),
    );
    m.finish()
}

/// Free-running maximal-length 16-bit Fibonacci LFSR
/// (x^16 + x^14 + x^13 + x^11 + 1); `out` is stage 15. The feedback
/// XOR tree is built from `std.xor` instances.
///
/// Patent-review boundary: this module exposes one state bit only. Do
/// not turn several state bits into a signed first-order shaped-noise
/// word without re-running the claim review recorded in
/// `../LEGAL_REVIEW.md`.
fn build_lfsr16(xor_gate: ModuleId, library: &ModuleLibrary) -> Module {
    let mut m = ModuleBuilder::new_canonical("std.lfsr16");
    let out = m.output("out");

    // Seed 0b1 (stage 0 starts at 1); all-zero is the lock-up state.
    let stages: Vec<_> = (0..LFSR_BITS).map(|bit| m.reg(bit == 0)).collect();
    for bit in 1..LFSR_BITS {
        m.wire(
            Endpoint::reg_q(stages[bit - 1]),
            Endpoint::reg_d(stages[bit]),
        );
    }
    // **The shift tail, not the register.** Each stage drives the next,
    // so this is a chain; but a lane carries one body and stage 0 is
    // seeded, so `Reg { init: true }` is a different kind from the
    // fifteen after it. Naming all sixteen would be a lie the
    // `record_chain` assertion would catch anyway.
    //
    // So the claim is the honest one — these fifteen cells are one
    // homogeneous repeated chain — and stage 0 stays its own box, where
    // its wire into the tail is exactly what a reader should see.
    structure::record_chain(
        &mut m,
        "shift tail",
        CellKind::Reg { init: false },
        stages[1..].to_vec(),
    );

    // feedback = s15 ^ s13 ^ s12 ^ s10.
    let (xa, xb, xy) = (
        port_id(library, xor_gate, "a"),
        port_id(library, xor_gate, "b"),
        port_id(library, xor_gate, "y"),
    );
    let xor_of = |m: &mut ModuleBuilder, left: Endpoint, right: Endpoint| -> Endpoint {
        let xor = m.instance(xor_gate);
        m.wire(left, Endpoint::inner(xor, xa));
        m.wire(right, Endpoint::inner(xor, xb));
        Endpoint::inner(xor, xy)
    };
    let x1 = xor_of(
        &mut m,
        Endpoint::reg_q(stages[15]),
        Endpoint::reg_q(stages[13]),
    );
    let x2 = xor_of(
        &mut m,
        Endpoint::reg_q(stages[12]),
        Endpoint::reg_q(stages[10]),
    );
    let feedback = xor_of(&mut m, x1, x2);
    m.wire(feedback, Endpoint::reg_d(stages[0]));

    m.wire(Endpoint::reg_q(stages[15]), Endpoint::module(out));
    m.finish()
}

/// XOR from four NANDs; returns the output endpoint.
fn build_xor(m: &mut ModuleBuilder, a: Endpoint, b: Endpoint) -> Endpoint {
    let n1 = m.nand();
    m.wire(a, Endpoint::nand_a(n1));
    m.wire(b, Endpoint::nand_b(n1));
    let n2 = m.nand();
    m.wire(a, Endpoint::nand_a(n2));
    m.wire(Endpoint::nand_y(n1), Endpoint::nand_b(n2));
    let n3 = m.nand();
    m.wire(b, Endpoint::nand_a(n3));
    m.wire(Endpoint::nand_y(n1), Endpoint::nand_b(n3));
    let n4 = m.nand();
    m.wire(Endpoint::nand_y(n2), Endpoint::nand_a(n4));
    m.wire(Endpoint::nand_y(n3), Endpoint::nand_b(n4));
    Endpoint::nand_y(n4)
}
/// **A binary32 multiply in NAND gates — flush-to-zero, saturating.**
/// `a × b → out`, all 32-bit words in the usual layout: bit 31 sign,
/// bits 30..23 the biased exponent, bits 22..0 the stored fraction.
///
/// It is **not IEEE-754**, and calling it that — which this comment did
/// — is the kind of half-truth that gets a native instruction
/// substituted for it on inputs where the two disagree. What it
/// implements, stated exactly:
///
/// - **Normal numbers**, with the implicit leading 1 restored.
/// - **Round to nearest, ties to even** — the same rule the hardware
///   uses, which is what makes a substitution legal at all on the
///   values it does handle.
/// - **Zero**, in and out: either operand zero gives zero, and a result
///   too small to be normal is flushed to zero rather than going
///   subnormal. The sign survives, so a negative underflow is negative
///   zero.
/// - **Saturation instead of infinity.** A result past the largest
///   finite value becomes the largest finite value, sign kept.
/// - **No NaN, no infinity, no subnormals** — neither accepted nor
///   produced.
///
/// So it agrees with a hardware `fmul` exactly on normal values whose
/// product is normal, and deliberately differs outside that. A JIT may
/// only substitute a native instruction for a circuit it **exactly**
/// equals, so a lowering for this has to guard the corners or the
/// caller has to guarantee they cannot arise.
///
/// The cost is the point of the exercise. See `fmul32_costs_what_it_costs`.
///
/// It is generic arithmetic, like the fixed-point multiply beside it.
fn build_fmul32(
    and_gate: ModuleId,
    or_gate: ModuleId,
    xor_gate: ModuleId,
    not_gate: ModuleId,
    full_adder: ModuleId,
    mul24: ModuleId,
    library: &ModuleLibrary,
) -> Module {
    let and = gate_ports(library, and_gate);
    let or = gate_ports(library, or_gate);
    let xor = gate_ports(library, xor_gate);
    let fa = adder_port_ids(library, full_adder);
    let not_a = port_id(library, not_gate, "a");
    let not_y = port_id(library, not_gate, "y");

    let mut m = ModuleBuilder::new_canonical("std.fmul32");
    let a: Vec<Endpoint> = (0..32)
        .map(|i| Endpoint::module(m.input(format!("a.{i}"))))
        .collect();
    let b: Vec<Endpoint> = (0..32)
        .map(|i| Endpoint::module(m.input(format!("b.{i}"))))
        .collect();
    let out: Vec<PortId> = (0..32).map(|i| m.output(format!("out.{i}"))).collect();
    let zero = Endpoint::reg_q(m.constant(false));
    let one = Endpoint::reg_q(m.constant(true));

    let gate2 = |m: &mut ModuleBuilder, module: ModuleId, ports: &GatePorts, x, y| -> Endpoint {
        let cell = m.instance(module);
        m.wire(x, Endpoint::inner(cell, ports.a));
        m.wire(y, Endpoint::inner(cell, ports.b));
        Endpoint::inner(cell, ports.y)
    };
    let invert = |m: &mut ModuleBuilder, x: Endpoint| -> Endpoint {
        let cell = m.instance(not_gate);
        m.wire(x, Endpoint::inner(cell, not_a));
        Endpoint::inner(cell, not_y)
    };

    // Sign: one XOR, and the only genuinely cheap part of a float.
    let sign = gate2(&mut m, xor_gate, &xor, a[31], b[31]);

    // Mantissas with the implicit leading one restored.
    let mant = |w: &[Endpoint]| -> Vec<Endpoint> {
        let mut bits: Vec<Endpoint> = (0..23).map(|i| w[i]).collect();
        bits.push(one);
        bits
    };
    let ma = mant(&a);
    let mb = mant(&b);

    let product = m.instance(mul24);
    for (i, bit) in ma.iter().enumerate() {
        m.wire(
            *bit,
            Endpoint::inner(product, port_id(library, mul24, &format!("a.{i}"))),
        );
    }
    for (i, bit) in mb.iter().enumerate() {
        m.wire(
            *bit,
            Endpoint::inner(product, port_id(library, mul24, &format!("b.{i}"))),
        );
    }
    let p: Vec<Endpoint> = (0..48)
        .map(|i| Endpoint::inner(product, port_id(library, mul24, &format!("p.{i}"))))
        .collect();

    // Two 24-bit mantissas in [1, 2) give a product in [1, 4), so bit 47
    // is set exactly when the result needs one shift right. That single
    // bit is the whole of normalisation.
    let carry_out = p[47];
    let mux = |m: &mut ModuleBuilder, sel: Endpoint, lo: Endpoint, hi: Endpoint| -> Endpoint {
        // sel ? hi : lo, from AND/OR/NOT rather than a mux module so this
        // needs nothing beyond the gates already borrowed.
        let nsel = invert(m, sel);
        let take_hi = gate2(m, and_gate, &and, sel, hi);
        let take_lo = gate2(m, and_gate, &and, nsel, lo);
        gate2(m, or_gate, &or, take_hi, take_lo)
    };

    // The 23 kept fraction bits, before rounding. Without normalisation
    // the leading one sits at bit 46, so the fraction is bits 45..23;
    // with it, one place up.
    let kept: Vec<Endpoint> = (0..23)
        .map(|i| {
            let lo = p[23 + i];
            let hi = p[24 + i];
            mux(&mut m, carry_out, lo, hi)
        })
        .collect();

    // **Round to nearest, ties to even** -- the same rule the hardware
    // uses, which is the whole reason this circuit exists. Truncating
    // would be a simpler circuit and an unsubstitutable one: a JIT may
    // only put a native instruction in place of a netlist it *equals*.
    //
    // Guard is the first discarded bit; sticky is whether anything below
    // it was set. Round up when guard is set and either something below
    // it was (so the value is above halfway) or the kept low bit is odd
    // (so a tie goes to even).
    let guard = {
        let lo = p[22];
        let hi = p[23];
        mux(&mut m, carry_out, lo, hi)
    };
    let sticky = {
        // Bits strictly below the guard. One more bit joins the sticky
        // when the product normalised, which is the bit the guard moved
        // off; folding it in with a mux keeps one OR chain instead of two.
        let mut acc = zero;
        for bit in p.iter().take(22) {
            acc = gate2(&mut m, or_gate, &or, acc, *bit);
        }
        let extra = gate2(&mut m, and_gate, &and, carry_out, p[22]);
        gate2(&mut m, or_gate, &or, acc, extra)
    };
    let tie_to_even = gate2(&mut m, or_gate, &or, sticky, kept[0]);
    let round_up = gate2(&mut m, and_gate, &and, guard, tie_to_even);

    // Increment the fraction. Its carry out means the mantissa wrapped
    // from all-ones to zero, which is a further doubling and so a further
    // increment of the exponent -- and the fraction is then correctly all
    // zero, so nothing else needs fixing.
    let mut carry = round_up;
    let mut frac: Vec<Endpoint> = Vec::with_capacity(23);
    for bit in kept.iter() {
        let cell = m.instance(full_adder);
        m.wire(*bit, Endpoint::inner(cell, fa.a));
        m.wire(zero, Endpoint::inner(cell, fa.b));
        m.wire(carry, Endpoint::inner(cell, fa.cin));
        carry = Endpoint::inner(cell, fa.cout);
        frac.push(Endpoint::inner(cell, fa.sum));
    }
    let mantissa_wrapped = carry;

    // Exponent: ea + eb - 127 + (1 if normalised). Done as
    // ea + eb + (-127 + carry) in 10 bits so the intermediate cannot wrap.
    let add10 =
        |m: &mut ModuleBuilder, x: &[Endpoint], y: &[Endpoint], cin: Endpoint| -> Vec<Endpoint> {
            let mut carry = cin;
            let mut sum = Vec::with_capacity(10);
            for i in 0..10 {
                let cell = m.instance(full_adder);
                m.wire(x[i], Endpoint::inner(cell, fa.a));
                m.wire(y[i], Endpoint::inner(cell, fa.b));
                m.wire(carry, Endpoint::inner(cell, fa.cin));
                carry = Endpoint::inner(cell, fa.cout);
                sum.push(Endpoint::inner(cell, fa.sum));
            }
            sum
        };
    let widen = |w: &[Endpoint]| -> Vec<Endpoint> {
        let mut bits: Vec<Endpoint> = (23..31).map(|i| w[i]).collect();
        bits.push(zero);
        bits.push(zero);
        bits
    };
    let ea = widen(&a);
    let eb = widen(&b);
    let sum_e = add10(&mut m, &ea, &eb, carry_out);
    // Subtract the bias: + (1024 - 127) mod 1024 = + 897, which in ten
    // bits is 1110000001.
    let bias: Vec<Endpoint> = [1, 0, 0, 0, 0, 0, 0, 1, 1, 1]
        .iter()
        .map(|bit| if *bit == 1 { one } else { zero })
        .collect();
    // The bias add has a spare carry-in, so the rounding's own carry
    // rides in there. It cannot be merged with `carry_out` above: both
    // can be set at once -- a product that normalised *and* then rounded
    // from all-ones up to zero is a further doubling, worth +2 in all.
    let exp10 = add10(&mut m, &sum_e, &bias, mantissa_wrapped);

    // A rounded exponent at or below zero underflows; bit 9 marks a
    // negative exponent. Check all ten bits for zero so exponent 256 is
    // still an overflow. Underflow flushes to zero; overflow saturates
    // at the largest finite value.
    //
    // "Largest finite" means exponent 254, not 255. Driving every
    // exponent bit high on overflow -- which is what this did at first
    // -- produces 255, and 255 is infinity when the fraction is zero and
    // NaN when it is not. `3e38 * 2` came out NaN from a circuit whose
    // doc comment promised saturation. Bit 0 of the exponent is
    // therefore forced low, giving 0xFE, and the fraction is forced high
    // below, which together are f32::MAX with the sign kept.
    let mut any_exp_bit = exp10[0];
    for bit in exp10.iter().skip(1) {
        any_exp_bit = gate2(&mut m, or_gate, &or, any_exp_bit, *bit);
    }
    let exp_zero = invert(&mut m, any_exp_bit);
    let underflow = gate2(&mut m, or_gate, &or, exp10[9], exp_zero);
    let not_under = invert(&mut m, underflow);
    // Overflow starts at 255, not at 256. 255 is not a large exponent
    // that happens to be out of range -- it *is* the encoding of
    // infinity and NaN, so a result landing on it has already left the
    // finite numbers. Testing only bit 8 misses it exactly, which is why
    // `3e38 * 2` (whose exponents sum to precisely 255) came out NaN.
    let mut all_ones = exp10[0];
    for bit in exp10.iter().take(8).skip(1) {
        all_ones = gate2(&mut m, and_gate, &and, all_ones, *bit);
    }
    let past = gate2(&mut m, or_gate, &or, exp10[8], all_ones);
    let overflow = gate2(&mut m, and_gate, &and, past, not_under);
    let exp: Vec<Endpoint> = (0..8)
        .map(|i| {
            let ceiling = if i == 0 { zero } else { one };
            let saturated = mux(&mut m, overflow, exp10[i], ceiling);
            let alive = invert(&mut m, underflow);
            gate2(&mut m, and_gate, &and, alive, saturated)
        })
        .collect();

    // Either operand zero gives zero. A float is zero when its exponent
    // field is zero, which is all this needs to look at because
    // subnormals are declared out of scope.
    let any_exp = |m: &mut ModuleBuilder, w: &[Endpoint]| -> Endpoint {
        let mut acc = w[23];
        for bit in w.iter().take(31).skip(24) {
            acc = gate2(m, or_gate, &or, acc, *bit);
        }
        acc
    };
    let a_live = any_exp(&mut m, &a);
    let b_live = any_exp(&mut m, &b);
    let live = gate2(&mut m, and_gate, &and, a_live, b_live);
    let not_under2 = invert(&mut m, underflow);
    let alive = gate2(&mut m, and_gate, &and, live, not_under2);

    for i in 0..23 {
        // Saturating means the largest finite value, whose fraction is
        // all ones -- not the overflowed product's fraction, which would
        // be an arbitrary bit pattern next to a saturated exponent.
        let capped = mux(&mut m, overflow, frac[i], one);
        let bit = gate2(&mut m, and_gate, &and, alive, capped);
        m.wire(bit, Endpoint::module(out[i]));
    }
    for i in 0..8 {
        let bit = gate2(&mut m, and_gate, &and, alive, exp[i]);
        m.wire(bit, Endpoint::module(out[23 + i]));
    }
    m.wire(sign, Endpoint::module(out[31]));
    m.finish()
}
#[cfg(test)]
mod multiplier_tests {
    use super::*;
    use crate::compile::flatten;
    use crate::interp::FlatSimulator;

    /// The multiplier has to multiply before anything built on it means
    /// anything. Checked at a small width so the exhaustive corners are
    /// cheap, then at the width an IEEE single actually needs.
    #[test]
    fn the_array_multiplier_multiplies() {
        let stdlib = Stdlib::build();
        for width in [4_usize, 8, 24] {
            let (m, row) = build_multiplier(
                &format!("t.umul{width}"),
                width,
                stdlib.full_adder,
                stdlib.and_gate,
                &stdlib.library,
            );
            let id = m.id;
            let mut library = stdlib.library.clone();
            library.insert(row);
            library.insert(m);
            let flat = flatten(id, &library).expect("flattens");
            let mut sim = FlatSimulator::new(&flat);

            let max = (1_u64 << width) - 1;
            let cases: Vec<(u64, u64)> = vec![
                (0, 0),
                (1, 1),
                (max, 1),
                (1, max),
                (max, max),
                (max / 3, 5),
                (7, 9),
                (max / 2, max / 2),
            ];
            for (x, y) in cases {
                for bit in 0..width {
                    assert!(sim.set_input(&format!("a.{bit}"), (x >> bit) & 1 == 1));
                    assert!(sim.set_input(&format!("b.{bit}"), (y >> bit) & 1 == 1));
                }
                sim.tick();
                let mut got = 0_u64;
                for bit in 0..2 * width {
                    if sim.output(&format!("p.{bit}")).expect("a product bit") {
                        got |= 1 << bit;
                    }
                }
                assert_eq!(got, x * y, "{width}-bit: {x} x {y}");
            }
        }
    }
}
