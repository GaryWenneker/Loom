# Command-line reference

**Version:** Loom 0.1 experimental alpha. Run `loom help` to view the installed CLI's own usage. Except for `init` and help, commands require a Loom worktree.

| Command | Current behavior |
|---|---|
| `loom init [path]` | Initialize a local repository |
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

`loom query --batch @CHANGESET_ID [--cache-bytes N]` reads JSON-line requests
and emits framed binary `cat` replies or JSON `files`/`stats` replies. It opens
existing storage read-only without acquiring the workspace lock. See the
[protocol and integrity contract](snapshot-query-protocol.md) before implementing
a client. For a legacy revision without a native root, prepare it explicitly
with `loom tree @CHANGESET_ID`.
