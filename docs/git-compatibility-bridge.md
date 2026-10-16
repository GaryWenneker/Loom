# Explicit Git compatibility bridge

Loom's native engine is independent of Git. When an operation is not yet available natively, users may **explicitly** invoke an installed Git executable:

```sh
loom git -- status
loom git -- log --oneline
loom git -- diff --stat
```

The command executes `git` with the supplied arguments, inherits the current working directory and terminal streams, and preserves Git's exit code. OS arguments are passed without a shell, including non-UTF-8 filenames on Unix. Missing Git is a clear error. It works in Git repositories even when no `.loom` directory exists. It does **not** translate Loom object IDs, branches or history into Git; it does not convert a Loom repository into a Git repository. A Git command operates on Git's repository state. Arguments are unrestricted, just as for direct Git use: Git aliases, hooks and commands may modify files or contact remotes. In a directory containing both `.git` and `.loom`, shared worktree changes can affect either tool's next snapshot. The bridge does not authorize or perform native history conversion. Git must be installed and discoverable in `PATH`.

There is intentionally **no automatic fallback** for `loom commit`, `loom merge` or other native commands: silent cross-engine substitution could commit different data or create diverging histories. This bridge is an interim compatibility path, not Git import/export, native remote sync, or lossless migration. For automated scripts, treat it as an explicitly selected backend.

Future adapter work must detect repository identity, negotiate capabilities, preserve metadata and map failures clearly before any automatic routing is considered. See `docs/git-capability-parity.md`.
