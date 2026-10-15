# Loom

Loom is an experimental, standalone **native version control system**, implemented in Rust.

It is **not built on Git** and is not an extension of AX. A separate Git compatibility bridge is planned.

> **Experimental alpha:** the repository and object format can change. Never use Loom as the only copy of important source code. Current version only supports regular UTF-8-named files under 64 MiB, without preserved executable bits or symlinks.

## Implemented today

- Portable native binary changeset and blob envelopes addressed by BLAKE3-256.
- Local immutable object storage with on-read integrity verification.
- SQLite branch refs with compare-and-swap protection.
- Cross-process advisory worktree write lock around local commits, merges, branch creation and switching. This reduces local races but does **not** make worktree materialization crash-atomic.
- Local full-worktree status, commits, log, branch creation/switching, basic conservative merges and fsck.
- A CLI named `loom`.
- Read-only `loom diff` for worktree changes and comparisons between branch snapshots; text patches use unified diff output, large/binary changes are reported without textual hunks.
- `loom cat BRANCH PATH` for binary-safe direct access to native snapshot files without checkout.
- Cross-platform compilation, tests and Rust dependency auditing in GitHub Actions.

Merge supports conservative three-way line merging when text changes do not overlap and refuses ambiguous edits or binary conflicts. **Rename-aware merging, full conflict resolution, rebases and crash-atomic worktree materialization are not yet supported.** This is **not** ready to replace Git for production.

## Build

Install Rust 1.85+ and run:

```shell
cargo build --release -p loom-cli
cargo test --workspace --all-targets
```

The binary is `target/release/loom`.

## Try it (throwaway files only)

```shell
mkdir loom-demo && cd loom-demo
../target/release/loom init
echo hello > hello.txt
../target/release/loom status
../target/release/loom commit -m "Initial snapshot" --author "Developer"
../target/release/loom branch create feature
../target/release/loom switch feature
../target/release/loom log
../target/release/loom fsck
```

Run `loom help` for the supported CLI. The example assumes the binary is accessible at the indicated relative path; otherwise use its absolute path or add it to `PATH`.

## Plan

The [architecture](docs/architecture.md), [native format](docs/format-v1.md), [ADRs](docs/adr-001-native-engine.md) and [roadmap](docs/roadmap.md) document decisions and remaining engineering.

Tracked delivery issues: [native safety](https://github.com/GaryWenneker/Loom/issues/1), [full local VCS](https://github.com/GaryWenneker/Loom/issues/2), [merges/recovery](https://github.com/GaryWenneker/Loom/issues/3), [server/agents](https://github.com/GaryWenneker/Loom/issues/4), [Git bridge](https://github.com/GaryWenneker/Loom/issues/5), [scale](https://github.com/GaryWenneker/Loom/issues/6), and [production hardening](https://github.com/GaryWenneker/Loom/issues/7).

## License

MIT.
