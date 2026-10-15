# Functional parity with Git, without copying Git's CLI

**Status:** contract/feature inventory; not a declaration that Loom already implements these capabilities.  
**Product rule:** *Git is the minimum functional baseline; Loom may use different commands, data structures and safer/smarter interaction patterns.* Loom remains independent of Git, AX and AI services. Git integration belongs in an optional adapter.

The question is **"Can the user complete the same workflow without losing data or history?"**, not "Does Loom have a subcommand with the same spelling?"

## Definitions for tracked parity

- **Native**: capability is provided directly by Loom and its native engine.
- **Native alternative**: same user outcome is achieved by a different and preferably better Loom workflow; equivalence requires tests.
- **Adapter**: Git-specific protocol/object operations are available through an optional Git bridge, with documented lossless/lossy/unsupported cases.
- **Unsupported / blocked**: capability is not yet delivered. Never quietly substitute weaker behavior.
- **Verified**: native/adapter behavior is merged into main, exercised by conformance tests and passes the declared release gates.

Only a **verified** capability counts towards completion. A proposal, issue, alias, mock CLI or architectural diagram does not.

## Capability inventory

| Git user outcome | Loom target | Current main (2026-10-15) |
|---|---|---|
| Initialize local repository | Native `init` | Initial alpha |
| Inspect staged/unstaged/untracked/ignored changes | One grouped `status` and `review`, structured JSON | Partial; `review` proposed |
| Stage complete files, directories or selected hunks | Native snapshot staging, recursive/hunk selection | Partial, individual paths only |
| Commit exactly selected changes | Native immutable changeset + operation log | Partial, opt-in staging |
| Amend/split/squash/edit history | Explicit operations with retained previous revisions | Missing |
| Inspect history and ancestry including merges | Native graph queries, log and visualization | First-parent log only |
| Inspect file content and compare revisions | Native `cat`, `diff`, `show`, path history and blame | Basic `cat` and `diff` |
| Create, rename, delete, protect and switch branches | Native transactional refs and safe workspace changes | Basic create/list/switch |
| Lightweight and annotated/signed tags | Native tags, verified metadata/signatures | Missing |
| Work in several branches simultaneously | Native isolated workspaces sharing immutable content | Missing |
| Restore file or reset/recover staging/worktree | Native targeted restoration with preview and recovery | Missing |
| Stash/shelve interrupted work | Named immutable work-in-progress snapshots | Missing |
| Rebase, cherry-pick, revert, merge with conflict resolution | Native DAG/transforms, resumable typed conflicts | Conservative merge only |
| Locate regressions (bisect) | Native revision search/bisect with checkpointed runs | Missing |
| Ignore rules and safe cleanup | Native ignore and explicit safe clean | Ignore in draft PR #12 |
| Hooks and workflow customization | Native extensibility/scripts, secure capability policy | Missing |
| Sparse checkout and partial retrieval | Native sparse workspaces + on-demand objects | Missing |
| Handle large and binary files | Native chunks/range reads | Limited to files under 64 MiB |
| Preserve symlinks, executable flags, unusual paths | Versioned portable native tree metadata | Missing |
| Clone, fetch, push, pull, list remotes | Native versioned secure remote protocol | Missing |
| Concurrent writers and recovery from divergence | Immutable commits + ref CAS + merge queue | Local locks/CAS only |
| Interoperate with existing Git services and repositories | Independent import/export/sync adapter | Missing |
| Inspect/repair repository, salvage history, prune | Native `fsck`, operation log, backups, GC | Basic `fsck` only |
| Sign/verify commits, refs and release provenance | Native signatures + optional Git signature fidelity | Missing |
| Support Git submodules, LFS, SHA variants and signed object formats | Explicitly tested Git-adapter feature matrix | Missing |
| Use machine-readable automation and IDE integrations | Stable JSON/SDK, safe errors, progress and cancellation | Missing |

This inventory covers common porcelain workflows and important Git interoperability. It does not imply Loom must implement each esoteric Git plumbing command verbatim; necessary low-level capabilities must be available via the native API or a well-tested adapter.

## Preferred Loom innovations

### 1. One safe change-review view

Instead of forcing the user to guess which comparison shows the next commit, provide `loom review`: two independent views ("ready to commit" and "still in workspace"), with optional `--json`. The existing `loom diff` stays available, and narrower `--staged` / `--unstaged` selectors are also valid.

Staging is an immutable snapshot: edits after `add` do not quietly change an already prepared commit. A single `status` must not misleadingly double-count the same unchanged staged file.

### 2. Reversible operations rather than destructive defaults

An operation log tracks previous refs and workspace states. Safe undo/redo can restore a previous state without mutating immutable commits. Where full recovery is impossible, the UI requires explicit confirmation and a verified backup. `reset --hard`-style destructive behavior is never an implicit default.

### 3. Agent-native isolated workspaces

Each developer/agent operates on isolated mutable state, shared immutable objects and fast snapshots. Direct path queries avoid expensive full checkouts. A server-side merge/landing queue performs CAS, checks policies and reports conflicts without losing the losing actor's changeset.

### 4. Flexible branching semantics

Users may make temporary changesets without permanent named branches. Published shared branch refs remain transactional. Multi-workspace support must be strictly isolated to avoid stage contamination or cross-agent writes.

### 5. Fast native history and content access

Derived native tree/path indexes and object caches are rebuildable. Revision/path/ancestry queries should not require repeatedly scanning all files or replaying enormous histories. Do not assume performance gains without published, reproducible benchmarks.

### 6. Git as an interoperability boundary

Import and export Git trees/history with accurate metadata and stable mapping where supported. Preserve original signed bytes when exact signature retention is promised. Git remains optional in the Loom engine, but users can safely move into and out of existing Git workflows.

## Capability-completeness gate

Before marking Loom a usable **Git replacement** for a declared environment:

1. The feature matrix must identify every everyday Git workflow, its native/adapter mechanism and support status.
2. A test suite executes equivalent scenarios against Git and Loom, checking **user outcomes and data integrity**, not CLI text.
3. Exceptions are documented explicitly, not described as parity.
4. Import/export and round-trip tests guard existing Git history, file modes, tags, signatures and refs for claimed subsets.
5. Safety, recovery, security and cross-platform gates from [release-gates.md](release-gates.md) still apply.

**Important:** "No functionality lost" is a release criterion. It is not a claim about the current experimental alpha.
