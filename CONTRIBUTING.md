# Contribution provenance policy

Contributions must be original or submitted with documented permission under
`MIT OR Apache-2.0`. Add a Developer Certificate of Origin sign-off using
`git commit --signoff` and retain the source URL, revision, copyright notice,
license and modification summary for adapted material.

Do not copy code, diagrams, prose, test vectors, fonts or other assets from a
paper, patent, product or repository without a compatible license. A change
that introduces a new technique must say in its pull request where the
technique comes from.

Run before review:

```text
cargo fmt --all -- --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo deny check licenses sources
```
