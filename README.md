# Loom

Loom is an experimental, standalone **native version control system**, implemented in Rust.

Loom has its own native storage engine and object format. A separate Git interoperability bridge is planned.

**Product direction:** Git is the **functional baseline**, not a syntax or architecture template. Loom may offer fewer steps, reversible operations and agent-safe workflows, but must eventually preserve equivalent user capabilities. See the [Git capability inventory](docs/git-capability-parity.md).

> **Experimental alpha:** the repository and object format can change. Never use Loom as the only copy of important source code. Current version only supports regular UTF-8-named files under 64 MiB, without preserved executable bits or symlinks.

## Implemented today

- Native v1 blob/changeset envelopes and optional hierarchical v2 tree objects addressed by BLAKE3-256. Existing v1 commit IDs remain unchanged. Historical tree roots use batched, copy-on-write first-parent deltas and reuse cached ancestor roots; unmodified subtrees retain their object IDs. Both explicit-staging commits and legacy commit-all commits now publish their native tree-root mappings atomically with their branch refs. Staged commits also update their staging and path indexes in the same transaction.
- Local immutable object storage with on-read integrity verification. The repository can also use caller-supplied durable `ObjectBackend` implementations; the CLI supports a separate object directory through `loom init --object-dir DIRECTORY` and `LOOM_OBJECT_DIR`. See [storage backends](docs/storage-backends.md) for binding, integrity and enumeration semantics.
- SQLite branch refs with compare-and-swap protection.
- Cross-process advisory worktree write lock around local commits, merges, branch creation and switching.
- Durable SQLite intent journal for switch/merge transitions: Loom validates the worktree, stages replacement files, replays incomplete operations upon reopening, and updates HEAD/refs only after materialization. Merge recovery also publishes its validated immutable tree-root mapping in the same SQLite transaction as the branchref and journal deletion.
- Local full-worktree status, commits, log, branch creation/switching, basic conservative merges and `fsck`, including checks for staged blob references and pending-recovery changesets.
- Opt-in persistent native staging index: `loom add PATH...`, `loom unstage PATH...`, frozen content snapshots and staged-only commits. Once enabled, every later commit requires explicit staging; existing repositories retain legacy commit-all behavior until their first `loom add`.
- Targeted `loom add` and staged status use the native derived branch path index when current; the index can be rebuilt from authoritative history if missing or stale. General worktree scans and indexed ancestry/blame remain separate performance work.
- A CLI named `loom`.
- Read-only `loom diff` for worktree changes and comparisons between branch snapshots; `loom diff --staged` and `loom diff --unstaged` show the two sides of the staging boundary independently.
- `loom review` displays both prepared commit content and remaining uncommitted workspace edits in one view. Text patches use unified diff output; large/binary changes are reported without textual hunks.
- `loom cat BRANCH PATH` for binary-safe direct access to native snapshot files without checkout.
- `loom cat @CHANGESET_ID PATH` reads a single file at a historical revision without checkout, with an optional, rebuildable result cache.
- `loom tree @CHANGESET_ID` publishes and prints the canonical immutable directory-tree root for an existing revision (experimental; metadata from v1 history cannot be reconstructed).
- `loom tree ls @TREE_ID [DIRECTORY]` and `loom tree cat @TREE_ID PATH` navigate immutable directory objects directly, without branches, SQLite history indexes or checkout.
- `loom files BRANCH|@CHANGESET_ID [PREFIX] [--null]` to enumerate tracked paths in a branch or immutable historical revision without checkout. Current branch tips use a derived index; historical tree queries build a reusable SQLite snapshot from first-parent changesets or a cached ancestor. `--null` is safe for automated filename processing.
- Cross-platform compilation, tests and Rust dependency auditing in GitHub Actions.

Merge uses the best common ancestor in the full commit DAG and supports conservative three-way line merging when text changes do not overlap. `loom merge-bases LEFT_BRANCH RIGHT_BRANCH` prints all undominated common ancestors. Criss-cross histories with multiple best merge-bases now use a bounded recursive virtual-base merge when all intermediate changes resolve without ambiguity. Conflicting virtual bases, competing file deletions, unrelated histories and unresolved binary/text edits are rejected without modifying branches or user files. If the target branch is an ancestor, Loom reports that it is already up to date; if the current branch is an ancestor of the source, a journaled fast-forward advances the ref without manufacturing a merge changeset. **Rename-aware merging, full conflict resolution and rebases are not yet supported. The recovery journal covers many process-crash cases, not abrupt power loss or all filesystem races.** This is **not** ready to replace Git for production.

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

The [architecture](docs/architecture.md), [implementation contracts](docs/implementation-contracts.md), [release gates](docs/release-gates.md), [v1 format](docs/format-v1.md), [v2 native tree objects](docs/format-v2-trees.md), [ADRs](docs/adr-001-native-engine.md) and [complete delivery roadmap](docs/roadmap.md) document decisions, priorities, dependencies and objective acceptance criteria. The roadmap distinguishes merged features from planned work.

Tracked delivery issues: [native safety](https://github.com/GaryWenneker/Loom/issues/1), [full local VCS](https://github.com/GaryWenneker/Loom/issues/2), [merges/recovery](https://github.com/GaryWenneker/Loom/issues/3), [server/agents](https://github.com/GaryWenneker/Loom/issues/4), [Git bridge](https://github.com/GaryWenneker/Loom/issues/5), [scale](https://github.com/GaryWenneker/Loom/issues/6), and [production hardening](https://github.com/GaryWenneker/Loom/issues/7).

## License

MIT.
