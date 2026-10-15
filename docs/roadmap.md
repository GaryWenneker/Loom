# Loom delivery roadmap

No implementation status should be inferred from a checklist unless CI and integration tests prove it.

## Phase 0 — Contracts and scaffolding
- [x] Standalone Cargo workspace and CI configuration
- [x] Native v1 envelope definition
- [ ] Golden vectors verified across platforms
- [ ] Full threat model and design review

## Phase 1 — Native storage
- [ ] Durable immutable objectstore with integrity checking
- [ ] SQLite refstore and CAS
- [ ] Crash-injection tests and recovery guarantees
- [ ] Production-grade GC, backup and migration rules

## Phase 2 — Local VCS
- [ ] Scan, status, commit, history, verify
- [ ] Branch creation and switching
- [ ] Fast incremental derived indexes
- [ ] Metadata-complete file model

## Phase 3 — Merge and recovery
- [ ] Deterministic three-way merge with exhaustive conflicts
- [ ] Rebase, revert, undo and merge graph operations
- [ ] Concurrency/fault model testing

## Phase 4 — Server and many agents
- [ ] Streaming RPC, version negotiation and credentials
- [ ] Secure remote object and ref services
- [ ] Parallel-agent workload and load tests

## Phase 5 — Git interoperability
- [ ] Isolated Git adapter
- [ ] Faithful import/export and OID mapping
- [ ] Incremental sync, safe push and compatibility suite

## Phase 6 — Scale
- [ ] Sparse and virtual workspaces
- [ ] Chunk manifests and shared caches
- [ ] Scalable database and objectstore adapters
- [ ] Reproducible performance baselines vs Git

## Phase 7 — Production hardening
- [ ] Platform-by-platform fault testing, fuzzing, fsck
- [ ] Backups, disaster recovery, authorization and security review
- [ ] Windows, macOS and Linux packages and documentation

**Gate:** never declare phases 4–7 complete from passing unit tests alone.
