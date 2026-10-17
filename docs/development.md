# Developer guide

## Workspace structure

- `crates/loom-core`: native format, object storage, SQLite refs, staging, merge and recovery.
- `crates/loom-cli`: user-facing CLI and integration tests.
- `docs`: contracts, ADRs, format, quality gates and roadmap.
- `.github/workflows/ci.yml`: Linux, Windows, macOS tests and security audit.

## Build and validate

```sh
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace --all-targets
```

Current CI needs `cargo fmt --all -- --check` rather than merely `cargo fmt --all`, and should use a committed `Cargo.lock` with locked builds. These improvements are tracked as open review findings.

## Invariants

- Objects are immutable; changing their bytes changes their ID.
- Publish refs only after referenced objects are validated and durable.
- Detect competing writers via CAS; do not overwrite user files on recovery.
- Treat object paths and worktrees as untrusted, especially symlinks and platform filename collisions.
- Reject noncanonical, oversized and corrupt input with bounded resource consumption.
- Keep disk/transport/API compatibility implications visible in each PR.
- Make reproducible correctness tests a prerequisite to performance optimizations.

The large `repository.rs` should be modularized by storage, refs, graph, staging, merge, and worktree responsibilities after correctness contracts are explicit. Refactoring alone is not proof of durability.

See [code review](code-review.md), [Scion architecture assessment](scion-architecture-review.md), and [release gates](release-gates.md).
## Reproducible Rust checks

Commit `Cargo.lock` for the shipped CLI. Repository Cargo configuration prefers
versions compatible with the workspace's declared Rust minimum. The CI matrix
uses `--locked`, checks formatting without changing source, denies Clippy
warnings, and verifies Rust 1.85. A newer dependency is not automatically a
compatible dependency: the initial minimum-version check caught globset 0.4.20
requiring Rust 1.88; the lock selects compatible 0.4.19.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```
