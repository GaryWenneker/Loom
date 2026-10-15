# Troubleshooting

## Not in a Loom repository

Run `loom init` in a new disposable directory. Other commands discover repository metadata by searching upward for `.loom/index.sqlite`.

## Already initialized

Do not delete `.loom` to silence the error. It contains native object history and SQLite metadata.

## Working tree has uncommitted or untracked files

Branch switching and merges require a clean worktree in the current alpha. Check `loom status` and `loom review`. Preserve unrelated files elsewhere before retrying; Loom does not yet provide a stable stash or restore workflow.

## Nothing staged to commit

The first `loom add PATH` enables explicit staging. Use `loom review` to inspect exactly which bytes will be committed. `loom unstage PATH` removes index entries without removing worktree files.

## Merge conflicts

Conservative merging refuses ambiguous edits and does not have `merge --continue` or `--abort`. Preserve both sides and use a separate copy for manual resolution.

## Recovery halted

A pending journal may refuse recovery if unexpected user edits exist. **Do not delete journal files or internal SQLite tables to force progress.** Back up the full worktree and `.loom` directory before collecting diagnostics.

## Unsafe repository metadata

Loom refuses a symlinked `.loom` directory, SQLite database or sidecar, and a symlinked `.loom/workspace.lock`. Do not bypass the error by removing or replacing metadata files. Preserve a complete copy of the repository and inspect the filesystem for unexpected links. This is a pre-open safety check; concurrent filesystem races remain outside the current alpha guarantees.

## Object integrity error

Stop mutating the affected repository, retain byte-for-byte copies and report the smallest reproducible case. Automatic repair and disaster recovery are not currently available.

## Unsupported files

Native v1 supports UTF-8 paths and regular files within the 64 MiB object cap. Symlinks, executable-bit preservation, unusual path encodings and large file manifests remain unsupported.

## Reporting a problem

Include the exact Loom commit SHA, OS, filesystem type, Rust version, minimal steps and sanitized logs. Report vulnerabilities privately via [SECURITY.md](../SECURITY.md).
