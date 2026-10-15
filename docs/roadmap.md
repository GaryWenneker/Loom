# Loom delivery roadmap (source of truth)

**Project:** Loom, a standalone native version control system in Rust. Its core owns a native object format and does not depend on Git. Interoperability is an optional external adapter.  
**Status:** experimental local alpha. Native format v1 is not stable; do not entrust irreplaceable source code to Loom.  
**Evidence rule:** `[x]` means code and tests are present on **main**, not merely that an issue, branch or PR exists. Verify CI for the actual merge SHA. Feature branches remain `[ ]` until merged.  
**Companion specifications:** [implementation contracts](implementation-contracts.md), [release gates](release-gates.md), [Git capability parity](git-capability-parity.md), [format v1](format-v1.md), and [architecture](architecture.md).

## Completion means four distinct milestones

1. **Alpha (current):** independent local content-addressed engine, basic CLI, conservative merge, staged snapshots and partial process-crash recovery. Not for critical data.
2. **Local beta:** format contract, portable metadata, full safe local workflows, no known loss of committed or uncommitted data under the defined fault model; verified Git migration round trips for declared supported features.
3. **Collaborative beta:** secure native sync protocol and remote ref CAS; at least 100 simultaneous agents tested under faults, with published latency/cost measurements.
4. **Production v1.0:** independent storage/security review, validated durability across OS/filesystems, migration and recovery drills, complete release/upgrade story and a documented compatibility contract. Advanced global scale is an optional follow-on, not an excuse to delay correctness.

## Non-negotiable invariants

- A canonical object's ID depends only on canonical bytes, never storage encoding, cache or transport.
- Immutable objects cannot be mutated; refs are atomically updated only after their transitive closure is verified and durable.
- Commit DAG is acyclic; the first parent is the delta base; a changeset remains reconstructable even if derived indexes are removed.
- All worktree-mutating operations serialize per workspace; ref updates use expected-old-value CAS. No loss of concurrent updates, no silent force-push.
- Recovery is idempotent; unknown user edits, untracked paths, symlinks and path collisions are never silently overwritten.
- Offline sync preserves divergent histories and reports conflicts; no last-writer-wins for user data.
- The Git adapter cannot introduce Git object IDs, Git assumptions or Git-only metadata into the native core.
- The protocol and format are versioned, fuzzed and bounded; malformed clients and repositories must not cause unsafe filesystem access or unbounded resource use.
- A data-preserving downgrade or a **verified backup/restore procedure** is mandatory before an incompatible migration.

## Dependencies and critical path

```text
P0 specification + P1 native durability
          |
P2 complete portable local VCS
          |
P3 correct merge, index & crash recovery
          |
          +---- P4 native server and secure sync ---+
          |                                        |
          +---- P5 Git migration / conformance -----+---- P6 agent concurrency, sparse workspaces
                                                      |
                                               P7 scale + storage backends
                                                      |
                                            P8 release hardening / production
```

P4 and P5 may be developed independently after the native state/metadata contract is stable. P7 performance optimizations must never precede correctness gates. Cross-cutting X1-X6 apply throughout.

## P0 — Format, policies and architecture contracts ([issue #1](https://github.com/GaryWenneker/Loom/issues/1))

**Implemented on main**
- [x] Standalone Rust workspace, basic architecture ADRs and three-platform CI.
- [x] Native v1 blob/changeset envelope using BLAKE3-256 and documented parser limits.

**To complete**
- [ ] Finalize object model v2: canonical `Blob`, `BlobManifest`, `Tree`, `Changeset`, `Tag`, `Conflict` and `OpRecord` schemas, type/version domain separation and deterministic ordering.
- [ ] Define exact byte-level encoding, field validation and golden hash vectors across Linux/macOS/Windows, including corrupted/noncanonical encodings.
- [ ] Define and implement capability negotiation, format compatibility windows, feature flags and downgrade boundaries. No silent interpretation of unknown fields.
- [ ] Decide path-identity policy (raw bytes or encoded portable form), Unicode normalization, case-collision handling, reserved names, symlinks and supported file modes; make workspace behavior explicit per platform.
- [ ] Write threat model, attack surfaces, trust boundaries, support matrix and data-loss failure taxonomy.
- [ ] Record ADRs for native trees/checkpoints, workspace transactions, offline/online sync, append-only operations log, server ref authority and hash-algorithm agility.

**Gate P0:** two independent implementations/test harnesses produce identical object-ID vectors; incompatible format changes are rejected clearly; threat model and migration ADR approved.

## P1 — Durable native storage ([issue #1](https://github.com/GaryWenneker/Loom/issues/1))

**Implemented on main**
- [x] Basic immutable loose-object store with on-read hash verification and `fsck`.
- [x] SQLite WAL metadata, conditional branch refs and cross-process advisory lock.
- [x] Initial persistent journal/replay around branch switch and merge.
- [x] Immutable staged blob snapshots.

**To complete**
- [ ] Make `init` rollback-safe; validate `.loom` and object paths against symlink/hardlink races, TOCTOU and permission surprises.
- [ ] Validate the **complete transitive closure** of all reachable objects and metadata before publishing refs, not just immediate parents.
- [ ] Define durable commit protocol for each backend: atomic rename, file/directory fsync, SQLite transaction boundary and post-crash observation; fault-test every boundary.
- [ ] Introduce corruption quarantine, read repair policy (when a trustworthy remote exists), clear error codes and explicit recovery tooling.
- [ ] Build reachability/pins/leases, GC with grace periods, concurrent reader/writer safety, pack compaction and bounded disk reclamation.
- [ ] Provide deterministic backup, point-in-time snapshot, restore and verification; validate that backups include refs, op log, metadata and required objects.
- [ ] Provide schema/format migrations with dry-run, backup requirement, progress, restart/resume, interruption rollback and explicit no-downgrade regions.
- [ ] Add streaming IO, bounded memory, quota/accounting and storage resource limits.

**Gate P1:** systematic `kill -9`, disk-full, rename/fsync/SQLite fault injection and backup/restore drills demonstrate that no published ref points to invalid or missing data within a documented platform-specific fault model.

## P2 — Complete local user workflow ([issue #2](https://github.com/GaryWenneker/Loom/issues/2))

**Implemented on main**
- [x] Basic `init/status/commit/log/branch/switch/diff/cat/fsck` and local end-to-end tests.
- [x] Opt-in persistent SQLite staging index, selected file snapshots and staged deletion.
- [ ] Unified `loom review` with separate staged and post-stage worktree diff views (in implementation PR; do not mark complete before merge).
- [x] Basic full-worktree scans and first-parent log.

**To complete**
- [ ] Land and verify root `.loomignore` support from [PR #12](https://github.com/GaryWenneker/Loom/pull/12); it is not considered released while open. Evaluate ignored-path behavior for safe switches and recovery.
- [ ] Define explicit staging as default for new repos, and a deliberate migration from legacy commit-all (avoid surprising mixed semantics). Add recursive `add`, `add -A`, `-p`, staged diffs, `reset/restore`, `rm` and `mv`.
- [ ] Add signed/annotated and lightweight tags, detached states, branch rename/delete, revision selectors and structured machine-readable `--json`.
- [ ] Implement native tree objects/checkpoints, path lookups, blame, ancestry index, fast status cache and crash-safe incremental invalidation.
- [ ] Preserve regular/executable file modes, symlink targets (never follow untrusted links), platform-supported filenames, empty directories policy, binary changes and file/directory transitions.
- [ ] Add large-file chunk manifests, content streaming, deduplication, range reads and per-object integrity; confirm >64 MiB files function.
- [ ] Establish portable CLI output, exit codes, shell completion, help examples, config hierarchy, author identity and sensible diagnostics.
- [ ] Decide tracked vs untracked vs ignored semantics for `switch`, `restore`, `merge` and `clean`; destructive operations require explicit confirmation/force flags with narrow scope.
- [ ] Multiple workspaces sharing one objectstore, isolated staging indexes/locks and worktree metadata.

**Gate P2:** clean and dirty state transitions, file modes/symlinks, binary data, partial staging, 100K-file fixtures and cross-platform roundtrips pass; unexpected user data is never overwritten.

## P3 — History, merge, rebase and recovery ([issue #3](https://github.com/GaryWenneker/Loom/issues/3))

**Implemented on main**
- [x] First-parent changeset replay and parent-DAG inspection.
- [x] Conservative two-parent merge and bounded non-overlapping text-line merge.
- [x] Cross-process local mutation lock and initial journal-based process-crash recovery.

**To complete**
- [ ] Implement correct DAG common ancestors with **multiple merge bases** and criss-cross merges; define recursive/virtual-base strategy and fast-forward semantics.
- [ ] Implement full typed three-way conflicts: edit/edit, delete/edit, add/add, rename/rename, rename/delete, binary, mode, symlink and file/directory collisions.
- [ ] Persist conflicts and support `merge --continue/--abort`, reproducible conflict markers, custom merge drivers and safe retries.
- [ ] Implement rebase, cherry-pick, revert, squash, bisect and an operation log supporting safe undo/redo; retain old unreachable heads for explicit recovery.
- [ ] Journal **all** destructive worktree mutations (not just switch/merge), with per-operation validation, atomic/staged writes, durable directory handling and recovery after partial execution.
- [ ] Model concurrency with CAS and lock granularity for multiple workspaces, processes and offline sessions; prove linearizability of ref mutations.
- [ ] Prove no uncommitted or unrelated ignored/untracked data is lost in fault injection; test restart after every write/rename/fsync/metadata transition.
- [ ] Bound DAG traversal depth/cycles, deep history evaluation, adversarial trees and memory/time use.

**Gate P3:** property-based DAG and merge tests, conflict fixtures, power-loss-inspired fault model, repeated abort/restart and concurrent process tests produce no lost user edits and fully recoverable published history.

## P4 — Native client/server and offline sync ([issue #4](https://github.com/GaryWenneker/Loom/issues/4))

- [ ] Versioned Protobuf/gRPC API: capability handshake, Object/Changeset/Ref/History/Diff/Workspace/Admin services, bounded streaming and explicit errors.
- [ ] Define **remote authority** for mutable refs; immutable object transport over HTTPS/TLS, checksum verification and missing-object discovery.
- [ ] Atomically publish refs only after transitive object verification and durable storage; per-repo CAS semantics across multiple servers.
- [ ] Implement authN (tokens/OIDC/mTLS where appropriate), least-privilege authZ, repository-scoped access, secret rotation, audit log and abuse limits.
- [ ] Design idempotent requests, resumable/ranged uploads, retry-after, backpressure, timeouts, resource quotas, cancellation and protocol feature negotiation.
- [ ] Local-first clone/fetch/pull/push and offline workflows with explicit divergent refs, leases, merges/rebases and no silent overwrite.
- [ ] Test network partitions, duplicate/reordered requests, simultaneous writers, server restarts and unsupported protocol peers.
- [ ] Ship self-hosted single-node deployment, TLS setup, install/update docs, health/readiness, logs, metrics and backups.

**Gate P5:** no unauthorized object leaks, no missing object closure, lossless offline divergence resolution, reproducible 100-writer race tests, secure interoperation between mixed supported versions.

## P5 — Git interoperability ([issue #5](https://github.com/GaryWenneker/Loom/issues/5))

- [ ] Build a **separate** `loom-git` crate/CLI adapter; no dependency from `loom-core` to Git formats or Git executables.
- [ ] Import all declared-supported blob/tree/commit/tag graphs with parents, merge structure, author/committer identity, timezone, messages, timestamps, file modes, symlinks and refs.
- [ ] Persist idempotent native-ID ↔ Git-OID mappings and preserve original Git object bytes/metadata when exact re-export is claimed.
- [ ] Define compatibility behavior for Git SHA-1 and SHA-256 repos, signed commits/tags, Git LFS pointers, submodules/gitlinks, shallow clones, replace refs, sparse checkout and unsupported extensions.
- [ ] Implement native → Git export and semantic round-trip tests; unsupported cases must fail explicitly rather than silently discard metadata or signatures.
- [ ] Implement GitHub/GitLab fetch/push via a distinct bridge, with ahead/behind state, remote ref CAS, no automatic force-push and recoverable interruptions.
- [ ] Add representative public-repo fixtures, signed fixtures and protocol conformance tests; benchmark migration disk and bandwidth.

**Gate P4:** declared feature matrix passes deterministic Git → Loom → Git and Loom → Git → Loom content/history tests; unsafe feature conversion is blocked; no destructive remote update by default.

## P6 — Agent-scale workspaces and developer platform ([issues #4](https://github.com/GaryWenneker/Loom/issues/4) and [#6](https://github.com/GaryWenneker/Loom/issues/6))

- [ ] Direct revision/path/blob APIs without checkout; paginated tree/diff/history queries, stable JSON/SDK schemas and predictable agent exit codes.
- [ ] Sparse native workspaces, checkout-on-demand, shared immutable local cache and optional cache daemon; no mandatory OS filesystem driver.
- [ ] Each agent/workspace receives independent staged changes and operation logs, with scoped permissions, resource quotas and optional TTL/lease.
- [ ] Safe concurrent changeset creation without global repo locks; centralized ref update CAS and optional landing/merge queue for shared branches.
- [ ] Define workspace cleanup, abandoned object retention, cancellation, retry, branch protection and audit trail for autonomous agents.
- [ ] FUSE/macOS/Windows virtual filesystem adapters only after sparse semantics and security have passed cross-platform tests.
- [ ] Test 1/10/100/500 concurrent actors; measure success rate, retries, conflict rate and correctness under simulated partitions.

**Gate P6:** 100 concurrent agents complete an agreed reproducible workload with **zero lost acknowledged commits** and no cross-workspace data contamination; p50/p95/p99, cache hit rate and resources published.

## P7 — Scale, distribution and operations ([issue #6](https://github.com/GaryWenneker/Loom/issues/6))

- [ ] Native tree/checkpoint and history indexes reduce path and ancestry queries below repeated full-history scans.
- [ ] Chunked/compressed object packing, manifest integrity, content-defined chunking evaluation (not assumed beneficial), pack compaction and GC leases.
- [ ] S3-compatible object backend and PostgreSQL (or equivalent) transactional ref backend, plus schema migration and backup consistency.
- [ ] Stateless read/RPC nodes, shared and regional caches, admission control, stampede protection, cache invalidation semantics and write authority.
- [ ] Replica consistency model, failover, read-after-write visibility, restore and partition behavior; declare supported topology explicitly.
- [ ] Metrics, traces, storage accounting and alerting for latency, corruption, GC, sync retries, saturation and quota exhaustion.
- [ ] Compare against Git on 10K/100K/1M-file synthetic and representative repos, full and sparse checkout, repeated small commits, long histories and 100+ agent writers; report raw scripts/data.

**Gate P7:** correctness gates continue to pass under distributed fault injection; benchmarks report disk, RAM, throughput, transferred bytes and p50/p95/p99 without ungrounded performance claims.

## P8 — Security, supply chain and release quality ([issue #7](https://github.com/GaryWenneker/Loom/issues/7))

- [ ] Independent threat model/security review, fuzz object decoders and RPC APIs, path traversal and symlink-race tests, malicious repo fixtures.
- [ ] Repo/branch permissions, protected refs, per-object authorization (including direct hash lookup and caches), credential rotation and auditable administrative actions.
- [ ] Pinned/verified CI actions, dependency update policy, `cargo audit`, dependency licenses, SBOM, provenance and signed/verifiable binary releases.
- [ ] Crash and disaster recovery drills, repeatable backup/restore, storage/format migrations, integrity scrubbing and retention/GC policy.
- [ ] Cross-platform packages (macOS Apple Silicon and Intel, Windows x64, Linux x64/ARM64 as supported), version reporting, uninstall/upgrade/rollback docs.
- [ ] Long-running soak/stress testing and published benchmarks, support policy, vulnerability reporting, license and contributor guide.
- [ ] Versioned operator, contributor, CLI, admin, SDK, migration and Git compatibility documentation; precise unsupported feature matrix.

**Gate P8 / production v1.0:** all mandatory checks in [release-gates.md](release-gates.md) satisfied, tested upgrade + restore, signed artifacts, security approval and documented platform support. A green `cargo test` alone does **not** qualify.

## Cross-cutting work required in every phase

- **X1 Contract governance:** all binary/network/CLI changes get an ADR or compatibility note and executable conformance tests.
- **X2 Correctness first:** prioritize property-based testing, deterministic clocks/IDs where needed, crash failpoints and model checking of mutable refs.
- **X3 Performance evidence:** profile and benchmark before claiming improvements or introducing optimizations.
- **X4 Documentation:** update README, architecture, usage, roadmap and status on the PR that changes behavior.
- **X5 Isolation:** Loom is self-contained and works offline without Git, a hosting provider, cloud account, AI model or a server for supported offline operations.
- **X7 Capability parity, not syntax parity:** Every common Git user outcome must have a verified Loom-native workflow or explicit adapter path. Prefer safer/reversible mechanisms and agent-native isolation; track gaps in [capability inventory](git-capability-parity.md).
- **X6 Release engineering:** run targeted PR checks and full Linux/macOS/Windows + security/integrity gates for merges and releases. Never merge with unknown, failed or stale required checks.

## Current in-flight work

- [PR #12: root ignore rules](https://github.com/GaryWenneker/Loom/pull/12) is **not** counted implemented until merged with green checks. Review ignored/untracked path collisions, staged and legacy commit flows, Windows filesystem behavior, malformed patterns and restore semantics.
- All remaining unchecked items in P0–P8 represent planned work, **not** completed features.
- The original seven phase issues remain tracking parents; create small child issues/PRs with exact acceptance tests for each deliverable.

## Recommended immediate order (next PRs)

1. Finish and review PR #12, fix any failing tests, and merge only with the latest all-green CI.
2. Define native v2 portable file/tree model **before** adding more one-off v1 behaviors; golden vectors + explicit migration strategy.
3. Harden worktree journal (path safety, crash failpoints, symlinks, filesystem durability) and staged index invariants.
4. Complete local tags/restore/revert, executable bits/symlinks and safe tree snapshots, then rich merge/undo operations.
5. Start Git adapter and native server work in parallel **only after** format and ref-transaction contracts are firm.
6. Add agent-scale workspaces and distributed storage, then production release hardening with objective evidence.

## Additional explicit gap trackers

- [#13 Native v2 object model + migration](https://github.com/GaryWenneker/Loom/issues/13) links P0/P1/P2, including portable modes/trees.
- [#14 No-data-loss fault matrix](https://github.com/GaryWenneker/Loom/issues/14) links P1/P3/P8, including user file protection.
- [#15 Git/protocol conformance matrix](https://github.com/GaryWenneker/Loom/issues/15) links P4/P5/P8, including signing, version negotiation and unsupported features.

**Do not call Loom a Git replacement in production before the local-beta, interoperability and release gates pass.**
