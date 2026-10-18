# Command-line reference

**Version:** Loom 0.1 experimental alpha. Run `loom help` to view the installed CLI's own usage. Except for `init` and help, commands require a Loom worktree.

| Command | Current behavior |
|---|---|
| `loom init [path]` | Initialize a local repository |
| `loom read-pack @CHANGESET_ID FILE` | Explicitly prepare an immutable read accelerator; never overwrite FILE or remove live objects |
| `loom status` | Show current branch, staged and remaining changes |
| `loom add PATH [PATH ...]` | Capture each selected file as an immutable staged snapshot |
| `loom unstage PATH [PATH ...]` | Unstage files without deleting worktree contents |
| `loom review` | View staged and later worktree changes together |
| `loom diff` | Compare worktree against HEAD |
| `loom diff --staged` | Compare staged content against HEAD |
| `loom diff --unstaged` | Compare worktree with staged content |
| `loom diff FROM TO` | Compare branch tip snapshots |
| `loom commit -m MESSAGE [--author NAME]` | Commit selected staged changes |
| `loom log [limit]` | Show first-parent commit history (default: 20) |
| `loom branch list` | List named local branches |
| `loom branch create NAME` | Create a local branch |
| `loom switch NAME` | Switch branches if workspace is clean |
| `loom merge NAME [-m MESSAGE]` | Conservative merge using the best DAG common ancestor. Performs journaled fast-forward for a descendant source branch, or a no-op when already up to date. A divergent branch merge creates a two-parent changeset. For criss-cross DAGs, recursively combines independent best merge-bases into a temporary virtual base and proceeds only if all intermediate and final edits resolve cleanly. Conflicting bases, ambiguous deletions and unrelated histories are rejected; no interactive conflict resolution. |
| `loom merge-bases LEFT_BRANCH RIGHT_BRANCH` | Print every best common ancestor (one immutable changeset ID per line, sorted); no checkout or branch mutation. Unborn/disconnected histories print nothing. |
| `loom cat BRANCH PATH` | Stream exact file bytes from a native snapshot |
| `loom cat @CHANGESET_ID PATH` | Read historical file bytes without checkout, using a rebuildable lookup cache. |
| `loom files BRANCH|@CHANGESET_ID [PREFIX] [--null]` | List tracked native files at a branch tip or exact historical changeset without checkout. An optional prefix selects exact paths or directory descendants. Text mode quotes filenames; `--null` emits NUL-separated paths for scripts. Historical trees are cached, rebuildable derived SQLite views. |
| `loom tree @CHANGESET_ID` | Build/publish a canonical immutable directory tree for the native revision and print its BLAKE3 root object ID, without modifying the worktree. May leave unused immutable nodes after interruption. |
| `loom sparse-export @CHANGESET_ID DESTINATION [PREFIX ...]` | Materialize an immutable full or prefix-selected snapshot into a new isolated directory; refuses existing destinations and writes a completion receipt only after synced files. |
| `loom portable-manifest regular|executable|symlink PATH` | Publish an additive canonical v2 portable-file manifest without modifying history, staging, worktree or refs. Regular and executable inputs stream in fixed 4 MiB chunks, including files beyond the v1 64 MiB object limit. |
| `loom portable-tree DIRECTORY [--workers N]` | Publish an additive canonical portable-v2 directory closure. Bounded workers prepare independent regular files; zero selects an adaptive request. Durable publication, live verification and canonical parent-tree publication remain ordered after worker join. History, staging, worktree and refs remain untouched. |
| `loom portable-export @TREE_ID DESTINATION [--workers N]` | Live-verify and materialize an immutable portable-v2 directory closure into a new isolated directory. Refuses existing destinations, bounds provider-aware file reads, preserves portable executable/symlink semantics where supported, syncs files and writes a completion receipt last. |
| `loom portable-verify @TREE_ID DIRECTORY [--workers N]` | Verify a completed portable workspace against its immutable root without mutation. Requires the exact directory set, bytes, portable modes, symlink targets and completion receipt; bounded workers recheck metadata and directory membership for concurrent changes. |
| `loom portable-migrate-plan @CHANGESET_ID [--workers N]` | Read-only preflight of one v1 revision. Replays and live-verifies the immutable v1 closure, rejects portable path collisions, and reports the exact regular-file portable-v2 root and closure counts without publishing objects, indexes, refs, staging or workspace state. |
| `loom portable-migrate-publish @CHANGESET_ID [--workers N]` | Runs the exact preflight, then durably and idempotently publishes its canonical portable-v2 chunk/manifest/tree closure through bounded batches. It live-verifies the result and does not change refs, indexes, staging, format state or the worktree. |
| `loom portable-migrate-checkpoint @CHANGESET_ID [--workers N]` | Publishes and live-verifies the exact closure, then conditionally records a durable prepared-state receipt mapping that immutable v1 revision to its portable root and closure counts. Identical retries succeed; a conflicting receipt is never replaced. This does not activate v2 or change refs, staging, format state or the worktree. |
| `loom tree ls @TREE_ID [DIRECTORY]` | List immediate entries of an immutable directory object by its native ID; includes object IDs and file/dir types. |
| `loom tree cat @TREE_ID PATH` | Read raw binary file bytes by traversing immutable directory objects, without accessing branchrefs, historical caches or the worktree. |
| `loom fsck` | Verify native objects, tree node references, native revision-to-tree mappings, refs, derived indexes, staging and recovery roots; **not** a backup or full filesystem recovery test |

The first `loom add` enables persistent explicit staging in v1. Earlier commit-all behavior is transitional. A missing tracked file can be staged as a deletion. Directory-wide staging and partial hunk selection are unavailable.

Commands exit nonzero on error and diagnostics go to stderr. Typed exit codes and structured JSON output are not implemented.

**Unavailable:** clone/fetch/push/pull/remote, tags, rebase, cherry-pick, revert, restore, stash, interactive conflicts, signing, full metadata preservation.

[Getting started](getting-started.md) · [Concepts](concepts.md) · [Support matrix](support-matrix.md).

## External filesystem objects

Place external storage outside the workspace and use an existing parent directory. Initialization rejects a provider inside its own worktree, including symlink aliases, to prevent the store from being tracked in snapshots.

Initialize with `loom init [path] --object-dir DIRECTORY`, or set `LOOM_OBJECT_DIR` before initialization. Set `LOOM_OBJECT_DIR` to that same object directory for every subsequent command. Prefer an absolute directory path so invocation from nested workspace directories resolves consistently. The workspace's refs, staging and journal remain under `.loom`; only immutable objects move. Missing explicit binding is an error. Setting the environment variable on an existing default-storage repository does not perform a migration. See [storage backends](storage-backends.md).

## Explicit Git fallback

`loom git -- GIT_ARGS...` invokes installed Git with the current directory, terminal streams and unmodified OS arguments. It preserves Git's exit code and works without a Loom repository. No shell is inserted. Native commands never silently fall back. This is explicit Git execution, not native history import/export; Git commands retain their normal filesystem/network side effects. See [Git compatibility bridge](git-compatibility-bridge.md).

## SQLite objects

`loom init [path] --object-db FILE` initializes with SQLite object storage. Set `LOOM_OBJECT_DB` to the same database for subsequent commands. Use an absolute path and an existing parent directory. Choose one of `LOOM_OBJECT_DIR` and `LOOM_OBJECT_DB`; setting both is an error. The native format and IDs are identical to filesystem objects. Refs and the workspace journal retain their local SQLite transaction boundary.

## Persistent immutable queries

`loom query --batch @CHANGESET_ID [--cache-bytes N] [--read-pack FILE]` reads JSON-line requests
and emits framed binary `cat` replies or JSON `files`/`stats` replies. It opens
existing storage read-only without acquiring the workspace lock. See the
[protocol and integrity contract](snapshot-query-protocol.md) before implementing
a client. For a legacy revision without a native root, prepare it explicitly
with `loom tree @CHANGESET_ID`.

For grouped native file access, send `{"id":1,"op":"cat_many","paths":["src/main.rs","src/lib.rs"]}`
to that session. The reply has indexed lengths followed by one binary payload.
The [grouped-read contract](snapshot-query-protocol.md#native-grouped-file-reads)
describes framing, budgets, missing paths and whole-group errors.
