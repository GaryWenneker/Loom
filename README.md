# Loom

Loom is an experimental, standalone **native version control system**, implemented in Rust.

Loom has its own native storage engine and object format. A separate Git interoperability bridge is planned.

**Product direction:** Git is the **functional baseline**, not a syntax or architecture template. Loom may offer fewer steps, reversible operations and agent-safe workflows, but must eventually preserve equivalent user capabilities. See the [Git capability inventory](docs/git-capability-parity.md).

> **Experimental alpha:** the repository and object format can change. Never use Loom as the only copy of important source code. Current version only supports regular UTF-8-named files under 64 MiB, without preserved executable bits or symlinks.

## Implemented today

- Portable native binary changeset and blob envelopes addressed by BLAKE3-256.
- Local immutable object storage with on-read integrity verification.
- SQLite branch refs with compare-and-swap protection.
- Cross-process advisory worktree write lock around local commits, merges, branch creation and switching.
- Durable SQLite intent journal for switch/merge transitions: Loom validates the worktree, stages replacement files, replays incomplete operations upon reopening, and updates HEAD/refs only after materialization.
- Local full-worktree status, commits, log, branch creation/switching, basic conservative merges and `fsck`, including checks for staged blob references and pending-recovery changesets.
- Opt-in persistent native staging index: `loom add PATH...`, `loom unstage PATH...`, frozen content snapshots and staged-only commits. Once enabled, every later commit requires explicit staging; existing repositories retain legacy commit-all behavior until their first `loom add`.
- A CLI named `loom`.
- Read-only `loom diff` for worktree changes and comparisons between branch snapshots; `loom diff --staged` and `loom diff --unstaged` show the two sides of the staging boundary independently.
- `loom review` displays both prepared commit content and remaining uncommitted workspace edits in one view. Text patches use unified diff output; large/binary changes are reported without textual hunks.
- `loom cat BRANCH PATH` for binary-safe direct access to native snapshot files without checkout.
- `loom files BRANCH [PREFIX] [--null]` to enumerate the tracked files in a branch without checkout. The current branch-tip index avoids replaying history where available; `--null` is safe for automated filename processing.
- Cross-platform compilation, tests and Rust dependency auditing in GitHub Actions.

Merge supports conservative three-way line merging when text changes do not overlap and refuses ambiguous edits or binary conflicts. **Rename-aware merging, full conflict resolution and rebases are not yet supported. The recovery journal covers many process-crash cases, not abrupt power loss or all filesystem races.** This is **not** ready to replace Git for production.

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
# Optional: opt into explicit staging before commit
# ../target/release/loom add hello.txt
../target/release/loom commit -m "Initial snapshot" --author "Developer"
../target/release/loom branch create feature
../target/release/loom switch feature
../target/release/loom log
../target/release/loom fsck
```

Run `loom help` for the supported CLI. The example assumes the binary is accessible at the indicated relative path; otherwise use its absolute path or add it to `PATH`.

## Documentation and architecture review

- [Getting started](docs/getting-started.md): build, initialize, stage and work with branches.
- [CLI reference](docs/cli-reference.md): available commands and current limitations.
- [Engineering code review](docs/code-review.md): ranked correctness and security findings.
- [Scion architecture assessment](docs/scion-architecture-review.md): where the current engine meets or falls short of agent-scale source control.
- [Roadmap and gates](docs/roadmap.md): planned features, dependencies, acceptance criteria.

## Project resources

- [Concepts](docs/concepts.md) and [support matrix](docs/support-matrix.md)
- [Troubleshooting](docs/troubleshooting.md) and [development guide](docs/development.md)
- [Contributing](CONTRIBUTING.md), [security policy](SECURITY.md) and [changelog](CHANGELOG.md)

## Plan

The [architecture](docs/architecture.md), [implementation contracts](docs/implementation-contracts.md), [release gates](docs/release-gates.md), [native format](docs/format-v1.md), [ADRs](docs/adr-001-native-engine.md) and [complete delivery roadmap](docs/roadmap.md) document decisions, priorities, dependencies and objective acceptance criteria. The roadmap distinguishes merged features from planned work.

Tracked delivery issues: [native safety](https://github.com/GaryWenneker/Loom/issues/1), [full local VCS](https://github.com/GaryWenneker/Loom/issues/2), [merges/recovery](https://github.com/GaryWenneker/Loom/issues/3), [server/agents](https://github.com/GaryWenneker/Loom/issues/4), [Git bridge](https://github.com/GaryWenneker/Loom/issues/5), [scale](https://github.com/GaryWenneker/Loom/issues/6), and [production hardening](https://github.com/GaryWenneker/Loom/issues/7).

## License

MIT.
