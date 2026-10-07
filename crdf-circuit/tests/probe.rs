//! Hierarchical net map + net-value probing.

use crdf_circuit::model::{Endpoint, ModuleBuilder, ModuleLibrary};
use crdf_circuit::{FlatSimulator, flatten_with_net_map};

/// Builds `inv` (a NAND wired as an inverter: `y = !a`) and `outer`
/// (which instantiates `inv` once, `x -> inv.a`, `inv.y -> z`).
fn library() -> (ModuleLibrary, Setup) {
    let mut inv = ModuleBuilder::new("inv");
    let a = inv.input("a");
    let y = inv.output("y");
    let g = inv.nand();
    inv.wire(Endpoint::module(a), Endpoint::nand_a(g));
    inv.wire(Endpoint::module(a), Endpoint::nand_b(g));
    inv.wire(Endpoint::nand_y(g), Endpoint::module(y));
    let inv_mod = inv.finish();
    let inv_id = inv_mod.id;

    let mut outer = ModuleBuilder::new("outer");
    let x = outer.input("x");
    let z = outer.output("z");
    let cell = outer.instance(inv_id);
    outer.wire(Endpoint::module(x), Endpoint::inner(cell, a));
    outer.wire(Endpoint::inner(cell, y), Endpoint::module(z));
    let outer_mod = outer.finish();
    let outer_id = outer_mod.id;

    let mut lib = ModuleLibrary::new();
    lib.insert(inv_mod);
    lib.insert(outer_mod);

    (
        lib,
        Setup {
            outer_id,
            x,
            z,
            cell,
            inner_a: a,
            inner_y: y,
            inner_g: g,
        },
    )
}

struct Setup {
    outer_id: crdf_circuit::model::ModuleId,
    x: crdf_circuit::model::PortId,
    z: crdf_circuit::model::PortId,
    cell: crdf_circuit::model::CellId,
    inner_a: crdf_circuit::model::PortId,
    inner_y: crdf_circuit::model::PortId,
    inner_g: crdf_circuit::model::CellId,
}

#[test]
fn net_map_addresses_ports_and_aliases() {
    let (lib, s) = library();
    let (_flat, map) = flatten_with_net_map(s.outer_id, &lib).unwrap();

    assert_eq!(map.root(), s.outer_id);
    assert_eq!(map.module_at(&[]), Some(s.outer_id));

    // An instance pin aliases the child occurrence's boundary port.
    let x_net = map.net(&[], Endpoint::module(s.x)).unwrap();
    let inst_a = map
        .net(&[], Endpoint::inner(s.cell, s.inner_a))
        .expect("instance pin resolves");
    assert_eq!(x_net, inst_a, "x is wired to inv.a — same net");

    // inv.y (inner NAND y) == inv's boundary y == outer z (all wired).
    let inner_y_net = map.net(&[s.cell], Endpoint::nand_y(s.inner_g)).unwrap();
    let inv_out = map.net(&[s.cell], Endpoint::module(s.inner_y)).unwrap();
    let z_net = map.net(&[], Endpoint::module(s.z)).unwrap();
    assert_eq!(inner_y_net, inv_out);
    assert_eq!(inv_out, z_net);

    // Unknown path / mismatched pin -> None.
    assert!(map.net(&[s.inner_g], Endpoint::module(s.x)).is_none());
}

#[test]
fn probed_net_values_track_logic() {
    let (lib, s) = library();
    let (flat, map) = flatten_with_net_map(s.outer_id, &lib).unwrap();
    let y_net = map.net(&[s.cell], Endpoint::nand_y(s.inner_g)).unwrap();

    let mut sim = FlatSimulator::new(&flat);
    assert!(sim.set_input("x", true));
    sim.tick();
    assert_eq!(sim.net_value(y_net), Some(false), "y = !x = !1 = 0");
    assert_eq!(sim.output("z"), Some(false));

    assert!(sim.set_input("x", false));
    sim.tick();
    assert_eq!(sim.net_value(y_net), Some(true), "y = !0 = 1");

    // Out-of-range net -> None.
    assert_eq!(sim.net_value(sim.net_count() as u32), None);
}

#[test]
fn owned_simulator_outlives_circuit() {
    let (lib, s) = library();
    let (flat, map) = flatten_with_net_map(s.outer_id, &lib).unwrap();
    let y_net = map.net(&[s.cell], Endpoint::nand_y(s.inner_g)).unwrap();

    // `from_owned` takes the circuit, so the simulator is `'static`.
    let mut sim = FlatSimulator::from_owned(flat);
    assert!(sim.set_input("x", true));
    sim.tick();
    assert_eq!(sim.net_value(y_net), Some(false));
}
