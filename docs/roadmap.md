# Loom delivery roadmap

Checklist status reflects implemented source and tests, not an architectural promise. Native format v1 remains experimental.

## Phase 0 — Contracts and scaffolding
- [x] Standalone Cargo workspace and cross-platform CI
- [x] Native v1 envelope definition and format documentation
- [ ] Published portable hash golden vectors and full design review
- [ ] Full threat model

## Phase 1 — Native storage
- [x] Basic immutable loose-object store with per-read hash verification
- [x] Embedded SQLite refstore with conditional updates
- [x] Basic reference and object integrity verification (`fsck`)
- [ ] Crash-injection tests and proven durability across OS/filesystems
- [ ] Garbage collection, backup and schema migration guarantees

## Phase 2 — Local VCS
- [x] Full-worktree scan, status, commit and first-parent log
- [x] Local branch creation and switching
- [x] CLI integration tests for a basic change/merge workflow
- [x] Read-only branch/worktree diff and direct snapshot file reads (`loom diff`, `loom cat`)
- [ ] Staging/index, ignores, tags, restores, fast derived history indexes
- [ ] Symlinks, modes, non-UTF-8 files and chunked large file support

## Phase 3 — Merge and recovery
- [x] Conservative two-parent merge: merges independent file edits
- [x] Conflicting file changes reject the merge without moving HEAD
- [x] Bounded, conservative textual three-way merge for independent lines (no rename detection)
- [ ] Full typed conflict model, conflict continuation and advanced merge cases
- [ ] Safe rebase, revert, undo and crash-recoverable workspaces
- [x] Advisory cross-process serialization of local worktree mutations, with CLI locking integration test
- [ ] Complete concurrency model tests and merge-graph property tests

## Phase 4 — Server and many agents
- [ ] Streaming RPC, version negotiation, credentials and authorization
- [ ] Secure remote object and ref services
- [ ] 100+ parallel agent write tests, simulated faults and latency measurements

## Phase 5 — Git interoperability
- [ ] Isolated Git import/export bridge and OID mapping
- [ ] Preserved supported metadata and comprehensive conversion suite
- [ ] Incremental sync, safe push, signing compatibility strategy

## Phase 6 — Scale
- [ ] Sparse and virtual workspaces
- [ ] Chunk manifests, packfiles and shared caches
- [ ] Transactional scalable database and objectstore adapters
- [ ] Reproducible performance baselines vs Git

## Phase 7 — Production hardening
- [ ] Platform-by-platform fault injection and fuzzing
- [ ] Backups, disaster recovery, access policies and security review
- [ ] Windows, macOS and Linux release packages
- [ ] Verified formatting/style policy and long-term compatibility guarantees

**Release rule:** Cross-platform passing unit tests do not establish production readiness.
