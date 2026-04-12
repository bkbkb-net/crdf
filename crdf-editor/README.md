# crdf-editor

A visual RDF graph editor built with [crdf](../crdf/) and [egui](https://github.com/emilk/egui).

## Features

- **Open / Save** — Load and save RDF graphs in N-Triples (`.nt`) and FlatBuffers (`.crdf`) formats.
- **Visual graph** — Force-directed layout with color-coded nodes (IRI, blank node, literal).
- **Edit** — Add and remove triples via a side panel form.
- **Undo / Redo** — Full undo/redo support for graph operations.
- **Pan & Zoom** — Navigate large graphs with mouse controls.
- **DPO Graph Rewriting** — Create and apply Double Pushout rewriting rules via the DPO panel (powered by [crdf-dpo](../crdf-dpo/)).

## Running

```sh
cargo run -p crdf-editor
```

## Using the DPO Panel

1. Open the DPO panel from the menu bar: **View → Show DPO Panel**.
2. Enter a rule name and click **➕ New Rule** to create a rule.
3. Build the rule by adding pattern triples to **L** (match pattern), **K** (preserved interface), and **R** (replacement):
   - Click **➕ Add Triple** under each section.
   - For each subject / predicate / object, select the type (**IRI**, **Literal**, or **?Var**) and enter the value.
   - Variables (e.g. `person`) are shared across L, K, and R — they bind to concrete terms when a match is found.
4. The panel shows live validation status (✓ or ✗) and lists all variables in the rule.
5. Use the action buttons:
   - **🔍 Find Matches** — Count how many matches exist in the current graph.
   - **👁 Preview** — Show which triples would be deleted (red) and added (green), without modifying the graph.
   - **▶ Apply** — Apply the rule to the first match.
   - **▶▶ Apply All** — Apply to all non-overlapping matches.

### Example: Relocate a person

Given a graph with `Alice livesIn Tokyo` and `Alice a Person`:

| Section | Subject | Predicate | Object |
|---------|---------|-----------|--------|
| **L** | ?person (Var) | `http://example.org/livesIn` (IRI) | `http://example.org/Tokyo` (IRI) |
| **L** | ?person (Var) | `rdf:type` (IRI) | `http://example.org/Person` (IRI) |
| **K** | ?person (Var) | `rdf:type` (IRI) | `http://example.org/Person` (IRI) |
| **R** | ?person (Var) | `http://example.org/livesIn` (IRI) | `http://example.org/Osaka` (IRI) |
| **R** | ?person (Var) | `rdf:type` (IRI) | `http://example.org/Person` (IRI) |

Clicking **▶ Apply** deletes `Alice livesIn Tokyo` and adds `Alice livesIn Osaka`.

## License

Licensed under either of [Apache-2.0](../LICENSE-APACHE) or [MIT](../LICENSE-MIT) at your option.
