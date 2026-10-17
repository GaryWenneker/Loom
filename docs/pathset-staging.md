# Atomic selected path sets

`loom add src docs/file` prepares one atomic set of regular files, directories and tracked deletions. `loom add .` explicitly selects the whole repository. Directory selections recurse only inside those directories; missing tracked descendants become deletions. Tracked file/directory replacements include both sides. Overlapping scopes are deduplicated, while explicit invalid or ignored file arguments still reject the operation.

```sh
loom add --scan-workers 8 src/module-a src/module-b/config.json removed-directory
loom diff --staged
loom commit -m "update selected modules"
loom unstage src/module-a
loom unstage .
```

`loom add --scan-workers N` selects a fixed bounded worker request for reproducible tests; zero or omission uses adaptive selection, and the engine caps excessive requests. `Repository::add_paths` and `add_paths_with_scan_options` provide the same operation to agents. The return value counts selected candidate paths, including unchanged paths. `unstage_paths` removes selected exact paths/subtrees in one transaction without reading working contents. Unstage keeps explicit staging mode enabled. Empty directories are not stored. Scope syntax supports `.`/`./`, relative canonical paths and trailing directory slashes; it does not implement Git pathspec wildcards, exclusions or force-add. Input scopes and candidate paths share the existing one-million-path ceiling; prefix deduplication uses indexed ancestor checks rather than pairwise comparisons.

## Data path

The worktree lock covers recovery, branch/index pinning, selection, object preparation and the final metadata transaction. The first selection on a missing/stale branch index reconstructs it from authoritative v1 history. This bootstrap is repository-sized and persists only rebuildable metadata. With a current index, exact keys and BINARY prefix ranges enumerate only selected tracked/staged paths. Unrelated directory contents are not walked. An external provider's existing reopen/closure checks may still be repository-sized.

Root ignore rules are loaded once. Untracked ignored descendants are skipped; existing tracked or prepared additions stay visible. Explicit ignored untracked files fail, including redundant explicit arguments covered by a broader scope. Existing staging outside the selection is retained. Removing an uncommitted prepared addition removes its stage entry rather than manufacturing an invalid deletion.

Read/hash preparation reuses the bounded full-content scanner: 1,024 files / 8 MiB discovered payload per window, one valid larger file serially, capped workers, and bounded read lengths. Unchanged selected content is not republished. Changed canonical buffers flow through normal conditional publication, sync and verified live readback. All object workers finish before staging updates.

One SQLite transaction checks the captured branch and ref, enables explicit staging, replaces selected changes and removes selected no-ops. An invalid selection, reader/provider error, moved branch or failed SQL update cannot partially replace the prepared snapshot or newly enable staging. Immutable orphan objects and derived index bootstrap changes can remain after failure; refs and preexisting staged content stay intact. A retry is idempotent. Commit subsequently freezes the prepared bytes, updates its native tree and publishes ref/root/path-index changes atomically under the existing protocol.

## Limits and verification

Selection metadata scales with candidate paths; the payload window is not a total RSS cap. Parent traversal retains the v1 filesystem race limitations documented in [worktree scans](worktree-scans.md). External edits are not an atomic filesystem snapshot; applications must supply the complete intended set. This is explicit change selection, not an automatic event journal or stat-cache authority. Legacy commit-all/status retain their full scans.

Tests cover multi-directory selection, unrelated oversized files, exact prefix boundaries, deletions, file/directory replacements, duplicate/root/ignored selections, immutable prepared bytes, atomic unstage, invalid-input and SQL rollback, provider/readback faults, moved refs and retry. Full fsck remains the integrity check for stored history.
