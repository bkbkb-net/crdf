# Contribution provenance policy

Contributions must be original or submitted with documented permission under
`MIT OR Apache-2.0`. Add a Developer Certificate of Origin sign-off using
`git commit --signoff` and retain the source URL, revision, copyright notice,
license and modification summary for adapted material.

Do not copy code, diagrams, prose, test vectors, fonts or other assets from a
paper, patent, product or repository without a compatible license. Changes to
noise generation, DSP, codecs, hardware protocols or JIT strategy must update
`LEGAL_REVIEW.md` when they introduce a new technique.

Run before review:

```text
cargo fmt --all -- --check
cargo test --workspace --all-features
cargo clippy -p crdf-circuit --all-targets --all-features -- -D warnings
cargo rustdoc -p crdf-circuit --all-features -- -D warnings
cargo deny check licenses sources
```
