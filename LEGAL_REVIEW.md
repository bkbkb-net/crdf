# CRDF workspace legal and provenance review

Review date: 2026-07-29. This is an engineering compliance record, not legal
advice, patent clearance or a guarantee of non-infringement. Patent scope is
determined claim-by-claim and country-by-country.

## Licensing disposition

The workspace and `crdf-circuit` are declared `MIT OR Apache-2.0`, and the
repository contains both complete license texts. Downstream users may select
either option. For substantial commercial integrations, Apache-2.0 may be the
more useful option because section 3 contains an express patent grant from
contributors for claims necessarily infringed by their contributions. That
grant is limited to contributor claims and terminates under the license's
patent-litigation condition; it is not general freedom to operate.

The resolved dependency graph was inspected with `cargo metadata --locked`.
No dependency forces this workspace to adopt a copyleft license:

- Cranelift/Wasmtime crates are `Apache-2.0 WITH LLVM-exception`.
- `crdf`, `crdf-circuit`, `crdf-dpo`, `crdf-editor` and local `crdt-graph`
  declare `MIT OR Apache-2.0`.
- `self_cell` offers `Apache-2.0 OR GPL-2.0-only`; the Apache option is used.
- The GUI's `epaint_default_fonts` includes OFL-1.1 and Ubuntu Font Licence
  assets. A binary editor distribution must ship those font license texts.
- Other resolved crates have an accepted permissive option listed in
  `deny.toml`.

Run `cargo deny check licenses sources` after every `Cargo.lock` change. For a
binary editor release, generate a complete third-party license artifact from
the exact lockfile rather than relying on any summary:

```text
cargo install --locked cargo-about
cargo about generate about.hbs -o THIRD_PARTY_LICENSES.html
cargo deny check licenses sources
```

Library crates do not bundle their dependencies' source into the crate archive,
but distributors of final binaries must preserve every required license,
copyright, patent, trademark and NOTICE item; package consumers of the library
crates must perform the same audit for their own binaries.

## Copyright and provenance

Copyright protects the particular source and documentation, not an abstract
algorithm or method. `crdf-circuit` is committed with its author data; keep
design notes, prompts/specifications and review records with it. Before accepting
outside contributions, require an origin statement and DCO sign-off.

The implementation uses Cranelift through its public Rust API; no Cranelift
source was copied. Standard-library circuits are expressed locally from NAND
and register primitives. Do not copy source, schematics, tables or prose from a
paper, patent or product merely because the underlying circuit is conventional.
Record the exact source and license of any future imported material.

## Preliminary patent screen

Screened features include NAND universality, registers, ripple/full adders,
array multiplication, multiplexers, bit-sliced evaluation, JIT compilation and
a 16-bit Fibonacci LFSR using polynomial `x^16 + x^14 + x^13 + x^11 + 1`.

The search found US7385537B2, "Linear feedback shift register first-order noise
generator." Its independent claim includes outputting a multi-bit
two's-complement first-order noise signal using a plurality of LFSR bit
positions. `std.lfsr16` exposes only stage 15 as a single Boolean output and
does not create such a word or frequency-shaped first-order output. Google
Patents reports US status active with adjusted expiration 2026-10-15, while
also warning that its status is not a legal conclusion. This preliminary
comparison is not a non-infringement opinion.

The source now records that boundary beside `build_lfsr16`. Do not expose the
whole LFSR state as a signed shaped-noise sample, or implement a sigma-delta
dither application from it, without a fresh professional claim review.

Searches by feature name did not identify a patent that can responsibly be
declared infringed by the local adders, multiplier or NAND evaluator. Keyword searching cannot establish absence of relevant claims,
continuations, equivalents or non-US family members. Before commercial release
or hardware implementation, search J-PlatPat, USPTO Patent Public Search and
each intended market, then retain patent counsel if the exposure justifies it.

### Source and provenance policy

There is generally no safe category called a “source with no copyright or
license.” Original code and documentation normally receive copyright
automatically unless a reliable public-domain dedication (for example CC0
where effective) or an explicit license says otherwise. The project therefore
uses this policy:

1. Read specifications, manuals and patent publications for facts, interfaces
   and prior-art investigation; cite them without copying expressive prose.
2. Write code, tests, names and UI locally from the project specification.
3. Import third-party code/assets only after recording author, exact revision,
   license, notices and compatibility; “no license file” means do not copy.
4. Treat an expired patent as prior-art evidence only after checking the
   official registers and family members in every release territory.
5. Treat an active patent publication as readable technical information, not
   permission to practise its claims.

## Release checklist

1. Preserve `LICENSE-MIT`, `LICENSE-APACHE` and all applicable upstream notices.
2. Run tests, Clippy, rustdoc and `cargo deny check licenses sources` against
   the locked dependency graph.
3. Generate third-party notices for any final binary, especially GUI fonts and
   Cranelift's LLVM exception.
4. Verify every contributed file's author, origin and license.
5. Re-run patent screening when adding codecs, shaped noise, hardware
   protocols or a materially different JIT strategy.
6. Do not state that this review is a patent clearance or legal opinion.

## Remaining engineering work

`CircuitEditor` retains its add/remove `RdfOperation` values for
replica broadcast. Optional future work includes large-asset serialization
benchmarks, schema migration when a v2 exists, off-thread circuit hot swapping
and domain-specific validation reports. These should be implemented only when a
consumer requirement is concrete.

## Sources consulted

- Apache License 2.0 text: <https://www.apache.org/licenses/LICENSE-2.0>
- GNU license compatibility list:
  <https://www.gnu.org/licenses/license-list.html#apache2>
- U.S. Copyright Office, computer programs:
  <https://www.copyright.gov/register/tx-programs.html>
- JPO patent term guidance:
  <https://www.jpo.go.jp/e/faq/yokuaru/patent.html>
- J-PlatPat search: <https://www.j-platpat.inpit.go.jp/s0100>
- USPTO Patent Public Search: <https://ppubs.uspto.gov/pubwebapp/>
- US7385537B2 record: <https://patents.google.com/patent/US7385537B2/en>
- Cranelift/Wasmtime source and license:
  <https://github.com/bytecodealliance/wasmtime>
- `cargo-deny` license checks:
  <https://embarkstudios.github.io/cargo-deny/checks/licenses/>
