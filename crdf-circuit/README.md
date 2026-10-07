# crdf-circuit

`crdf-circuit` is a general-purpose synchronous digital-circuit engine
whose editable netlists are persisted as CRDF graphs.

The computational core has exactly two primitives:

- `nand`: two inputs and one zero-delay output;
- `reg`: a one-tick register with an initial bit.

Reusable modules, ports and immutable wires provide composition. The
compiler flattens module hierarchy, rejects undriven or multiply-driven
nets and combinational cycles, and produces a bit-sliced program that
evaluates 64 independent lanes at once. The optional `jit` feature uses
Cranelift to compile the same program to native code.

## Example

```rust
use crdf_circuit::{
    CircuitAsset, CircuitState, CompiledCircuit, Endpoint, ModuleBuilder,
    ModuleLibrary,
};

let mut module = ModuleBuilder::new("not");
let input = module.input("x");
let output = module.output("y");
let nand = module.nand();
module.wire(Endpoint::module(input), Endpoint::nand_a(nand));
module.wire(Endpoint::module(input), Endpoint::nand_b(nand));
module.wire(Endpoint::nand_y(nand), Endpoint::module(output));

let root = module.id();
let mut library = ModuleLibrary::new();
library.insert(module.finish());
let asset = CircuitAsset::new(root, 1, library);

let circuit = CompiledCircuit::compile(&asset)?;
let input = circuit.input_index("x").unwrap();
let output = circuit.output_index("y").unwrap();
let mut state = CircuitState::new(&circuit);
state.set_level(&circuit, input, 0, true);
state.process_step(&circuit);
assert!(!state.output_bit(output, 0));

# Ok::<(), Box<dyn std::error::Error>>(())
```

## CRDF editing and persistence

`asset_to_rdf` and `write_asset_into` serialize circuit assets into an
`RdfGraph`. `write_asset_into` is idempotent, rejects conflicting
definitions and leaves the destination unchanged on failure. A graph may
contain multiple assets that share canonical standard-library modules. Use
`write_asset_into_with_operations` when the bulk import must also be
broadcast to replicas.

Use `CircuitEditor` for in-place edits so stable entity identities and
CRDT history are preserved. Wires are immutable: replacing a connection
means removing the old wire and adding a new one. The editor retains every
`RdfOperation` in causal order; broadcast `operations()` or move completed
batches out with `take_operations()` and apply them on replicas with
`RdfGraph::apply_downstream`.

Standalone `.crdf` files preserve the complete underlying CRDT operation
history. Domain-specific conventions such as a fixed port-naming ABI
belong in consumer crates rather than this engine.

## Generic selectors

`std.select16` selects one of sixteen inputs through a NAND-derived mux tree.
Generic gates, adders, multipliers and stateful LFSRs can be composed into
consumer-owned circuits; domain-specific modules belong to the consumer.

## Features

- `jit` (default): enable the Cranelift native-code backend.

The interpreter and reference simulator remain available without default
features.

## License

Licensed under either Apache-2.0 or MIT, at your option.
See the workspace [legal and provenance review](../LEGAL_REVIEW.md) before
redistribution or changes to the circuit algorithms.
