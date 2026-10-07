//! A module can say a backend may replace it, and the claim survives
//! flattening.
//!
//! Nothing here knows what any intrinsic name means, which is the
//! point: the engine carries an opaque key and takes no view on it.

use crdf_circuit::compile::{CompileError, flatten};
use crdf_circuit::model::{
    Cell, CellId, CellKind, Endpoint, IntrinsicDecl, Module, ModuleId, ModuleLibrary, Port,
    PortDirection, PortId, Wire, WireId,
};

/// `y = !(a & b)`, wrapped in a module so it can be instantiated and so
/// it can carry a declaration.
fn nand_module(name: &str) -> Module {
    let mut m = Module::new(name);
    let a = PortId::new_random();
    let b = PortId::new_random();
    let y = PortId::new_random();
    m.ports = vec![
        Port {
            id: a,
            name: "a".into(),
            direction: PortDirection::Input,
        },
        Port {
            id: b,
            name: "b".into(),
            direction: PortDirection::Input,
        },
        Port {
            id: y,
            name: "y".into(),
            direction: PortDirection::Output,
        },
    ];
    let cell = CellId::new_random();
    m.cells = vec![Cell {
        id: cell,
        kind: CellKind::Nand,
    }];
    m.wires = vec![
        Wire {
            id: WireId::new_random(),
            from: Endpoint::module(a),
            to: Endpoint::nand_a(cell),
        },
        Wire {
            id: WireId::new_random(),
            from: Endpoint::module(b),
            to: Endpoint::nand_b(cell),
        },
        Wire {
            id: WireId::new_random(),
            from: Endpoint::nand_y(cell),
            to: Endpoint::module(y),
        },
    ];
    m
}

/// Two occurrences of `inner`, side by side, driven from the top's own
/// inputs. The second gets `b` on both pins, so its two declared input
/// nets come out equal.
fn top_with_two(inner: ModuleId, lib: &ModuleLibrary) -> Module {
    let mut m = Module::new("top");
    let a = PortId::new_random();
    let b = PortId::new_random();
    let y0 = PortId::new_random();
    let y1 = PortId::new_random();
    m.ports = vec![
        Port {
            id: a,
            name: "a".into(),
            direction: PortDirection::Input,
        },
        Port {
            id: b,
            name: "b".into(),
            direction: PortDirection::Input,
        },
        Port {
            id: y0,
            name: "y0".into(),
            direction: PortDirection::Output,
        },
        Port {
            id: y1,
            name: "y1".into(),
            direction: PortDirection::Output,
        },
    ];
    let inner_ports = &lib.get(inner).expect("inner").ports;
    let (ia, ib, iy) = (inner_ports[0].id, inner_ports[1].id, inner_ports[2].id);

    let mut wires = Vec::new();
    for (index, out) in [y0, y1].into_iter().enumerate() {
        let cell = CellId::new_random();
        m.cells.push(Cell {
            id: cell,
            kind: CellKind::Instance { module: inner },
        });
        let left = if index == 0 { a } else { b };
        wires.push(Wire {
            id: WireId::new_random(),
            from: Endpoint::module(left),
            to: Endpoint::inner(cell, ia),
        });
        wires.push(Wire {
            id: WireId::new_random(),
            from: Endpoint::module(b),
            to: Endpoint::inner(cell, ib),
        });
        wires.push(Wire {
            id: WireId::new_random(),
            from: Endpoint::inner(cell, iy),
            to: Endpoint::module(out),
        });
    }
    m.wires = wires;
    m
}

/// Declares the whole module, which is the only shape flattening
/// accepts: every port on one side or the other.
fn declare(m: &mut Module, name: &str, revision: u32) {
    let side = |want: PortDirection| -> Vec<PortId> {
        m.ports
            .iter()
            .filter(|p| p.direction == want)
            .map(|p| p.id)
            .collect()
    };
    m.intrinsic = Some(IntrinsicDecl {
        name: name.into(),
        revision,
        inputs: side(PortDirection::Input),
        outputs: side(PortDirection::Output),
    });
}

/// **A declaration reaches the flattened netlist, per occurrence.**
///
/// Two instances of the same module are two regions. That is the whole
/// difficulty: `Origin` records the module a gate is written in, so it
/// cannot tell one occurrence from another, and a backend substituting
/// for one has to know which nets are that one's.
#[test]
fn each_occurrence_becomes_its_own_region() {
    let mut lib = ModuleLibrary::new();
    let mut inner = nand_module("thing");
    declare(&mut inner, "vendor.thing", 3);
    let inner_id = inner.id;
    lib.insert(inner);
    let top = top_with_two(inner_id, &lib);
    let top_id = top.id;
    lib.insert(top);

    let flat = flatten(top_id, &lib).expect("flattens");
    let regions = flat.intrinsics();
    assert_eq!(regions.len(), 2, "one region per occurrence");

    for region in regions {
        assert_eq!(region.name, "vendor.thing");
        assert_eq!(region.revision, 3);
        assert_eq!(region.module, inner_id);
        assert_eq!(region.path.len(), 1, "one instance cell deep");
        assert_eq!(region.inputs.len(), 2);
        assert_eq!(region.outputs.len(), 1);
        assert!(!region.has_state, "no registers in a NAND");
    }
    assert_ne!(
        regions[0].path, regions[1].path,
        "the two occurrences are distinguishable, which Origin could \
         not do -- it names the module a gate is written in, and both \
         gates are written in the same one"
    );

    // The surrounding circuit tied the second occurrence's inputs
    // together, and the region reports that rather than hiding it.
    assert_eq!(
        regions[1].inputs[0], regions[1].inputs[1],
        "both inputs come from the same net"
    );
    assert_ne!(regions[0].inputs[0], regions[0].inputs[1]);

    // Every gate is claimed, and by its own region.
    assert_eq!(flat.gates().len(), 2);
    let mut claimed: Vec<_> = flat.gates().iter().map(|g| g.region).collect();
    claimed.sort();
    assert_eq!(claimed, vec![Some(0), Some(1)]);
}

/// **Declaring an intrinsic changes nothing about what a circuit
/// computes.**
///
/// The gates stay, driving the same nets. A backend with no lowering
/// for the name runs them and is right. This is the property that makes
/// a declaration safe to add to anything.
#[test]
fn a_declaration_does_not_change_the_netlist() {
    let mut plain_lib = ModuleLibrary::new();
    let inner = nand_module("thing");
    let inner_id = inner.id;
    plain_lib.insert(inner.clone());
    let top = top_with_two(inner_id, &plain_lib);
    let top_id = top.id;
    plain_lib.insert(top.clone());

    let mut marked_lib = ModuleLibrary::new();
    let mut marked_inner = inner;
    declare(&mut marked_inner, "vendor.thing", 1);
    marked_lib.insert(marked_inner);
    marked_lib.insert(top);

    let plain = flatten(top_id, &plain_lib).expect("flattens");
    let marked = flatten(top_id, &marked_lib).expect("flattens");

    assert!(plain.intrinsics().is_empty());
    assert_eq!(marked.intrinsics().len(), 2);
    assert_eq!(plain.gate_count(), marked.gate_count());
    for (p, m) in plain.gates().iter().zip(marked.gates()) {
        assert_eq!((p.a, p.b, p.y), (m.a, m.b, m.y), "same netlist");
    }
}

/// **A declaration inside another is ignored.**
///
/// A backend that can compute the whole has no use for a substitute for
/// a part of it, and a gate belonging to two regions would leave open
/// which one gets to skip it.
#[test]
fn the_outermost_declaration_claims_the_gates() {
    let mut lib = ModuleLibrary::new();
    let mut inner = nand_module("thing");
    declare(&mut inner, "vendor.thing", 1);
    let inner_id = inner.id;
    lib.insert(inner);
    let mut top = top_with_two(inner_id, &lib);
    declare(&mut top, "vendor.pair", 1);
    let top_id = top.id;
    lib.insert(top);

    let flat = flatten(top_id, &lib).expect("flattens");
    assert_eq!(
        flat.intrinsics().len(),
        1,
        "only the outer declaration survives"
    );
    assert_eq!(flat.intrinsics()[0].name, "vendor.pair");
    for gate in flat.gates() {
        assert_eq!(gate.region, Some(0));
    }
}

/// **A declaration that does not match its module is refused.**
///
/// Silently ignoring it would be worse than failing: the circuit would
/// run correctly and slowly, and nothing would say why.
#[test]
fn a_malformed_declaration_is_an_error() {
    // Every port accounted for, no duplicates -- but two of them on
    // the wrong side.
    let mut lib = ModuleLibrary::new();
    let mut inner = nand_module("thing");
    inner.intrinsic = Some(IntrinsicDecl {
        name: "vendor.thing".into(),
        revision: 1,
        inputs: vec![inner.ports[0].id, inner.ports[2].id],
        outputs: vec![inner.ports[1].id],
    });
    let inner_id = inner.id;
    lib.insert(inner);
    let top = top_with_two(inner_id, &lib);
    let top_id = top.id;
    lib.insert(top);

    assert!(matches!(
        flatten(top_id, &lib),
        Err(CompileError::IntrinsicPortDirection { .. })
    ));

    // And a port that is not on the module at all.
    let mut lib = ModuleLibrary::new();
    let mut inner = nand_module("thing");
    inner.intrinsic = Some(IntrinsicDecl {
        name: "vendor.thing".into(),
        revision: 1,
        inputs: vec![inner.ports[0].id, PortId::new_random()],
        outputs: vec![inner.ports[2].id],
    });
    let inner_id = inner.id;
    lib.insert(inner);
    let top = top_with_two(inner_id, &lib);
    let top_id = top.id;
    lib.insert(top);

    assert!(matches!(
        flatten(top_id, &lib),
        Err(CompileError::IntrinsicUnknownPort { .. })
    ));
}

/// **A declaration survives a save and a load.**
///
/// `Structure` may be lost without changing anything — it is
/// provenance. This is not: a compiler reads it. If it vanished on
/// reload, the same circuit would run at two different speeds depending
/// on whether it had been through a file, which is the kind of thing
/// that is discovered months later and never reproduced on demand.
#[test]
fn a_declaration_survives_a_round_trip() {
    use crdf::RdfGraph;
    use crdf_circuit::model::CircuitAsset;
    use crdf_circuit::{asset_from_rdf_by_id, write_asset_into};

    let mut lib = ModuleLibrary::new();
    let mut inner = nand_module("thing");
    declare(&mut inner, "vendor.thing", 7);
    let inner_id = inner.id;
    let declared = inner.intrinsic.clone().expect("just declared");
    lib.insert(inner);
    let top = top_with_two(inner_id, &lib);
    let top_id = top.id;
    lib.insert(top);

    let asset = CircuitAsset::new(top_id, 1, lib);
    let asset_id = asset.id;
    let mut graph = RdfGraph::new();
    write_asset_into(&mut graph, &asset).expect("writes");

    let back = asset_from_rdf_by_id(&graph, asset_id).expect("reads the asset back");
    let reloaded = back
        .library
        .get(inner_id)
        .expect("the module came back")
        .intrinsic
        .clone()
        .expect("and so did its declaration");
    assert_eq!(reloaded, declared);

    // And it still reaches the netlist from the reloaded library.
    let flat = flatten(back.root, &back.library).expect("flattens");
    assert_eq!(flat.intrinsics().len(), 2);
    assert_eq!(flat.intrinsics()[0].revision, 7);
}

/// **A declaration that leaves a port out is refused.**
///
/// This was a real hole, and the worst kind. Module hierarchy does
/// guarantee that a net inside an instance reaches the outside only
/// through that instance's ports — so I reasoned the region was closed
/// for free and wrote no check. The guarantee is about the *module's*
/// ports; it says nothing about whether the declaration listed them
/// all. Declare a NAND's `a` and `y` but not `b`, and `b` is a signal
/// crossing the boundary that no backend is told about: it would skip
/// the gates and compute from an input it never received. Wrong
/// answers, silently, only in the accelerated path.
#[test]
fn a_declaration_must_account_for_every_port() {
    let mut lib = ModuleLibrary::new();
    let mut inner = nand_module("thing");
    inner.intrinsic = Some(IntrinsicDecl {
        name: "vendor.thing".into(),
        revision: 1,
        inputs: vec![inner.ports[0].id],
        outputs: vec![inner.ports[2].id],
    });
    let inner_id = inner.id;
    lib.insert(inner);
    let top = top_with_two(inner_id, &lib);
    let top_id = top.id;
    lib.insert(top);

    assert!(matches!(
        flatten(top_id, &lib),
        Err(CompileError::IntrinsicPortsIncomplete { .. })
    ));

    // Listing one twice does not make up the count either.
    let mut lib = ModuleLibrary::new();
    let mut inner = nand_module("thing");
    inner.intrinsic = Some(IntrinsicDecl {
        name: "vendor.thing".into(),
        revision: 1,
        inputs: vec![inner.ports[0].id, inner.ports[0].id],
        outputs: vec![inner.ports[2].id],
    });
    let inner_id = inner.id;
    lib.insert(inner);
    let top = top_with_two(inner_id, &lib);
    let top_id = top.id;
    lib.insert(top);

    assert!(matches!(
        flatten(top_id, &lib),
        Err(CompileError::IntrinsicPortsIncomplete { .. })
    ));
}

/// **Saving twice leaves one declaration, not two.**
///
/// The first version minted a fresh subject for the declaration on
/// every write, so a second save added a second one and a merge of two
/// replicas that disagreed produced two subjects rather than one
/// conflict. The triples hang on the module's own subject now: a module
/// has at most one declaration and so needs no identity of its own.
#[test]
fn saving_a_declaration_twice_is_idempotent() {
    use crdf::RdfGraph;
    use crdf_circuit::model::CircuitAsset;
    use crdf_circuit::{asset_from_rdf_by_id, write_asset_into};

    let mut lib = ModuleLibrary::new();
    let mut inner = nand_module("thing");
    declare(&mut inner, "vendor.thing", 2);
    let inner_id = inner.id;
    lib.insert(inner);
    let top = top_with_two(inner_id, &lib);
    let top_id = top.id;
    lib.insert(top);

    let asset = CircuitAsset::new(top_id, 1, lib);
    let asset_id = asset.id;
    let mut graph = RdfGraph::new();
    write_asset_into(&mut graph, &asset).expect("writes");
    let after_first = graph.len();
    write_asset_into(&mut graph, &asset).expect("writes again");
    assert_eq!(graph.len(), after_first, "a second save adds nothing");

    let back = asset_from_rdf_by_id(&graph, asset_id).expect("reads the asset back");
    assert_eq!(
        back.library
            .get(inner_id)
            .expect("the module")
            .intrinsic
            .as_ref()
            .expect("the declaration")
            .revision,
        2
    );
}
