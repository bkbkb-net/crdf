# crdf-editor

A visual RDF graph editor built with [crdf](../crdf/) and [egui](https://github.com/emilk/egui).

## Features

- **Open / Save** — Load and save RDF graphs in N-Triples (`.nt`) and FlatBuffers (`.crdf`) formats.
- **Visual graph** — Force-directed layout with color-coded nodes (IRI, blank node, literal).
- **Edit** — Add and remove triples via a side panel form.
- **Undo / Redo** — Full undo/redo support for graph operations.
- **Pan & Zoom** — Navigate large graphs with mouse controls.

## Running

```sh
cargo run -p crdf-editor
```

## License

Licensed under either of [Apache-2.0](../LICENSE-APACHE) or [MIT](../LICENSE-MIT) at your option.
