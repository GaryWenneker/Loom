# Native ignore rules

Loom recognizes an optional root-level `.loomignore` file. This intentionally limited syntax supports basename globs (`*.log`), root-anchored globs (`/build/**`) and directory rules (`cache/`). Empty lines and comments are ignored. Negation, backslash escapes and nested ignore files are not supported; unsupported syntax produces an explicit error instead of silently ignoring a path.

Ignore rules apply only to **untracked** files during worktree status, worktree diff, unstaged diff, legacy commit-all and explicit staging. Already-tracked paths remain visible, even if a rule later matches them. `.loomignore` itself remains visible and can be committed.

Recovery and branch-switch safety scans deliberately do **not** filter ignored paths: ignoring a user file must not authorize overwriting it. This means an ignored large file or ignored untracked collision can still block a branch switch. Directory traversal is not pruned by ignore rules; filtering happens at individual file entries. These are deliberate safety limits rather than complete Git-compatible ignore semantics.

The native repository format, immutable tree identities and historical objects are unchanged by this feature.
