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
| `loom merge NAME [-m MESSAGE]` | Conservative merge; unresolved conflicts reject |
| `loom cat BRANCH PATH` | Stream exact file bytes from a native snapshot |
| `loom cat @CHANGESET_ID PATH` | Read historical file bytes without checkout, using a rebuildable lookup cache. |
| `loom files BRANCH|@CHANGESET_ID [PREFIX] [--null]` | List tracked native files at a branch tip or exact historical changeset without checkout. An optional prefix selects exact paths or directory descendants. Text mode quotes filenames; `--null` emits NUL-separated paths for scripts. Historical trees are cached, rebuildable derived SQLite views. |
| `loom tree @CHANGESET_ID` | Build/publish a canonical immutable directory tree for the native revision and print its BLAKE3 root object ID, without modifying the worktree. May leave unused immutable nodes after interruption. |
| `loom fsck` | Verify native objects, tree node references, native revision-to-tree mappings, refs, derived indexes, staging and recovery roots; **not** a backup or full filesystem recovery test |

The first `loom add` enables persistent explicit staging in v1. Earlier commit-all behavior is transitional. A missing tracked file can be staged as a deletion. Directory-wide staging and partial hunk selection are unavailable.

Commands exit nonzero on error and diagnostics go to stderr. Typed exit codes and structured JSON output are not implemented.

**Unavailable:** clone/fetch/push/pull/remote, tags, rebase, cherry-pick, revert, restore, stash, interactive conflicts, signing, full metadata preservation.

[Getting started](getting-started.md) · [Concepts](concepts.md) · [Support matrix](support-matrix.md).
