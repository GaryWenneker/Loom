# Loom implementation contracts and architecture decisions

**Purpose:** engineering contract for a new, standalone native version control system written in Rust. This document describes **target behavior**, not a claim that it is already implemented. For implemented status and sequencing see [roadmap](roadmap.md).

## 1. Product and compatibility boundaries

- Loom works offline as an embedded Rust engine plus CLI; an optional server provides collaboration and scale. The two modes use the **same** native object and transaction semantics.
- No dependence on Git, AX, proprietary cloud services, or AI models in `loom-core`. Git/GitHub/GitLab integration is an isolated adapter. AI agents are ordinary clients.
- Source control primitives are the product: no mandatory issue tracker, code-review UI, hosting service or CI runner.
- macOS/Linux/Windows are first-class; do not promise a feature on a platform without a validated filesystem contract for it.
- Acceptable native performance must be reproducibly demonstrated; the 200x infrastructure figure associated with other VCS systems is **not** a Loom target or claim.
- A protocol-compatible client must receive explicit errors for missing capabilities, permission denials, rejected conflict resolution and data integrity failures.

## 2. Canonical native object model (planned format v2)

### Object identity
`ObjectID = Hash(immutable domain tag || object-kind || schema-version || canonical-payload)`.
Use BLAKE3-256 initially and preserve room for a future algorithm identifier. The hash **does not** depend on compression, location, cache, timestamp of upload, or server architecture.

Hash domain separation prevents a payload of one type from being confused with another. Canonical payload encoding has exactly defined integer byte-order, string/byte encoding, field ordering, maximum sizes and validation; publish independent golden vectors. No permissive/ambiguous parsing of multiple byte representations for one logical object.

### Canonical records
| Type | Required semantics |
|---|---|
| `Blob` | Exact byte content of small regular files |
| `BlobManifest` | Ordered immutable chunk references, sizes, length and whole-content validation |
| `Tree` | Sorted directory entries, portable path component bytes, file type, executable flag/mode, object reference; no duplicate/colliding names |
| `Changeset` | Ordered parent IDs, author + committer identities with timezones, commit message, changed-file metadata/delta (first parent), extensible immutable metadata |
| `Tag` | Target object and optional annotated/signed identity |
| `Conflict` | Structured typed conflict state that can be resumed or aborted without losing original sides |
| `OpRecord` | User-visible append-only operation journal for ref/workspace mutation and undo, distinct from the crash-recovery journal |

Mutable refs (branches, release tag refs, workspace pointers) are **not** immutable content objects. Refs live in a transactional refstore and use expected-old CAS.

### Trees and changesets
Changesets form a DAG. Parent 0 defines delta application; other parents preserve merge ancestry. The resolved state can be materialized by replay plus a **rebuildable** native tree/checkpoint index. Tree/checkpoint cache eviction must not make native history unreconstructable. A snapshot query should not have to replay a million commits: use bounded indexed ancestry/path lookups and periodic snapshot materialization.

Merge bases may be multiple. A shortest-path-distance ancestor selection is **not** sufficient for a correct general-purpose merge algorithm.

### Filesystem portability
Define path identity separately from presentation. Specify whether native names are byte arrays with platform validation or a universal escaped encoding; document case folding and Unicode normalization collisions. Reject any tree whose paths cannot be safely materialized on the selected target OS. Never follow stored symlink contents as a path when reading or writing objects. Specify file modes/executable bits; directories and symlinks must not be silently coerced into regular files.

Avoid user data loss from untracked files, ignored content, hardlinks, symlink races, mount points, case-insensitive collisions or file/directory transitions. Document that ownership, ACLs, extended attributes and creation times are **not** portable unless explicitly supported.

### Format migrations
Format v1 is experimental. Before v2 migration:
1. Publish a detailed v1 → v2 translation contract, reversible where possible.
2. Check full repository integrity and free-space requirements.
3. Take and **verify** a restore-capable backup.
4. Rewrite objects with stable mappings and staged new refs; do not rewrite in-place.
5. Atomically publish new refs only when closure is complete.
6. Support interruption/restart and bounded old-format retention.
7. Explicitly refuse downgrade where it would discard data/metadata.

## 3. Storage, refs and transactional workspaces

### Local mode
- Immutable objectstore: loose files initially; packed/chunked content later. Verify hash/type/length on every untrusted read.
- SQLite WAL for local refs, staging metadata and operation logs. Ref CAS and logical op-log record in one DB transaction.
- Per-workspace advisory write lock; no dependency on ambient CWD or uncontrolled filesystem watchers for correctness.
- Worktree transition intent stored durably **before** mutation, with expected source/destination state, materialization plan and recoverability metadata.
- Verify and stage all destination object bytes, ensure storage durability, then replace worktree files; finalize HEAD/ref and journal cleanup atomically.
- Treat process termination, filesystem corruption, sudden power loss and user edits during a transition as **distinct** failure modes with separately tested guarantees.
- Do not overwrite unexpected paths on replay. If an operation cannot be safely replayed, retain the journal and expose a structured repair/abort procedure, plus a way to recover user data.
- Ensure stale staging directories are safely reclaimable, never delete active workspaces or journals.

### Remote mode
- Immutable objects uploaded first; validate the full reachable object closure before any ref update.
- Authoritative ref service commits `CAS(name, expected, next)`, not client-side last-writer-wins.
- Idempotency keys protect retried writes; operation logs capture actor, request and outcomes.
- Repository leases/pins protect objects involved in uploads, merges, offline clients and active workspaces from GC.
- Cross-server consistency: define read-after-write guarantees, staleness bounds for replicas, durable commit acknowledgment and safe failover.
- Shared object cache is untrusted: validate content IDs and apply **authorization before serving bytes**, including cache hits.

## 4. Workspaces and staging

- A workspace represents a revision plus independent staged/unstaged changes. Multiple workspaces may share immutable objects, **not** mutable indexes, HEADs or writer locks.
- `add` stages immutable file-content snapshot IDs. `commit` publishes those staged changes only. Stage/commit against stale HEAD must detect races and retain data for user resolution.
- Staging semantics must be uniform for new repos; use a documented migration path for opt-in v1 repos.
- `status` distinguishes staged, unstaged, untracked, ignored and conflicted files. Ignore rules never make an already tracked file disappear.
- `switch`, `restore`, `merge`, `reset`, `clean`, `rm` and `rebase` require explicit safe treatment of staged/untracked/ignored paths.
- Agent workspaces can be ephemeral but must have independent identities, leases, quotas and robust cleanup; no mandatory virtual filesystem.

## 5. Merge and history semantics

- Correct merge-base selection for criss-cross/multi-parent DAGs, not just one arbitrary common ancestor.
- Deterministic three-way merges; identical inputs always produce identical result/conflict objects.
- Explicit typed conflict matrix covering text, binary, addition/removal, mode, rename, symlinks and file-vs-directory.
- `merge --continue`, `--abort`, `rebase`, `cherry-pick`, `revert` and `undo` must not mutate published immutable objects. Rebase creates new IDs and preserves old identities via operation logs/retention.
- Fast-forward and already-ancestor cases are defined separately, with branch/ref CAS to prevent lost concurrent updates.
- Model-check or property-test ref transitions and DAG outcomes, including aborted workflows and malicious cycle/parent inputs.

## 6. Native network protocol and collaboration

**gRPC/Protobuf candidate interface** (versioned and negotiated):
- `Capabilities`: protocol version, supported formats/hash IDs/features, auth requirements.
- `Objects`: discover missing IDs, batched/ranged stream read, chunk upload, verified finalize.
- `Changesets`: graph retrieval, publish immutable changesets, metadata validation.
- `Refs`: list, watch, conditional update, audit events.
- `History/Diff`: indexed ancestry, merge-base, path history, blame and diff with pagination/limits.
- `Workspace`: sparse manifest, lease, materialization plan.
- `Admin`: repository configuration, backup/GC, health, quotas, policies.

All mutating requests require actor authentication, authorization, size/rate limits, cancellation and idempotency. Streams must enforce total-byte ceilings and timeouts. Never include sensitive tokens or private object contents in logs. Support TLS, key/token rotation and version skew during rolling upgrades.

**Offline workflow:** local immutable changesets and refs remain independent. On reconnect, exchange object reachability, compare remote head with expected old head, and either fast-forward or explicitly preserve divergent heads for user/merge queue. Never silently choose the newer timestamp.

## 7. Git adapter contract

Git is a translation boundary, not native storage.

- Import Git commit DAG, tree structure, author/committer identity, timestamps/timezones, modes, symlinks, refs, annotated tags and required extension metadata.
- Persistent `Git OID ↔ Loom ID` mapping is idempotent; repeated imports do not produce duplicate native history.
- Export is required to reconstruct an equivalent tree and parent graph. Exact Git SHA preservation needs retention of the original object bytes, and must **never** be promised after semantic rewriting.
- Preserve signed commit/tag objects when possible; when impossible, fail explicitly or produce an opt-in transformed export with signature invalidation made prominent.
- LFS pointers, submodules/gitlinks, shallow history, SHA-256 Git, signed tags/commits, merge histories and replace refs require individual compatibility statuses: supported, conversion, blocked or out-of-scope.
- Support GitHub/GitLab remote push only with branch-protection-aware CAS, no default force. Interrupted pushes must be retryable without losing native data.
- Validate with deterministic fixtures and representative real repositories; document expected semantics for any unavoidable metadata differences.

## 8. Distribution and performance architecture

- Begin with single-node correctness; split read service/cache/GC worker only when measured contention justifies it.
- A scalable refstore needs transactional consistency and CAS; object storage needs immutable read-after-write behavior or barriers before ref publication.
- Cache objects at client/edge/regional layers with checksums, leases/TTLs and authorization checks; no trust in cache metadata alone.
- Incremental tree/checkpoint indexes, change path indexes and stat caches support small commits without full-repository scans.
- Sparse checkouts and on-demand direct file reads precede OS-mounted virtual filesystems. FUSE/macOS/Windows drivers are optional separate modules.
- Design distributed GC around ref reachability, upload leases, checkpoint retention and concurrent pack rewrites.
- Benchmark full + sparse 10K/100K/1M-file workloads with 1/10/100/500 agents, p50/p95/p99 latency, bytes moved, peak RAM, CPU and disk; publish scripts and source fixture generation.

## 9. Security and operations

**Attack surfaces:** malicious repositories, attacker-controlled object IDs/paths, tampered blobs, untrusted RPC clients, auth bypass in cache, repository isolation, symlink/TOCTOU attacks, malicious Git metadata, compromised dependencies, accidental destructive CLI operations.

Minimum protections:
- Strict parser bounds, fuzzing and type-specific cryptographic domain separation.
- AuthN/AuthZ on every remote object/metadata endpoint; protected refs/roles and audit trail.
- Rate limiting, quotas, backpressure, object decompression bomb limits and safe log redaction.
- Dependency update/supply-chain policy, pinned CI, vulnerability disclosure, SBOM and signed artifacts.
- Verified backup/restore, recovery drill, migration/rollback policy and incident response documentation.
- A clear supported-platform matrix and a data-retention/GC policy.

## 10. API governance and backward compatibility

- Maintain versioned schemas and golden vectors for disk objects, protocol, CLI JSON and SDK APIs.
- Protocol capability negotiation rejects unsupported combinations rather than silently reducing guarantees.
- Public API deprecation period and migration strategy are required before v1.0.
- Distinguish compatible compression/index changes from incompatible **object meaning** changes.
- Freeze the stable disk format only after P0–P3 conformance; never label format v1 stable retroactively.

## 11. Explicitly deferred from first stable release

Integrated GitHub-style hosting, code-review web UI, CI service, global multi-region active-active writes, automatic LLM-assisted merges, semantic code indexing, and mandatory OS kernel filesystem drivers. These are optional future products, not dependencies of a safe independent VCS.

## 12. Architecture review checklist

Before any feature PR merges:
1. Does it preserve the canonical object/ref/workspace invariants?
2. Are disk/protocol/CLI compatibility implications documented?
3. Could a crash or race lose committed, staged, uncommitted, ignored or untracked data?
4. Does the feature behave safely on the supported OS filesystems?
5. Is its authZ/secret/logging surface reviewed (if server-related)?
6. Are adversarial and recovery tests present?
7. Do benchmarks justify added complexity?
8. Do docs describe what is **actually** implemented and what remains experimental?

See [release gates](release-gates.md) for the enforceable quality policy.
