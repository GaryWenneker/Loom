# Loom quality, acceptance and release gates

This document defines **evidence required**, not a claim of passing tests. Loom is experimental until the gates for its declared release tier pass. All gates are checked against an **exact Git commit SHA** and supported platform matrix.

## 1. Gate classes

| Gate | Every feature PR | Local beta | Collaborative beta | Production v1.0 |
|---|---|---|---|---|
| Rust `fmt`, Clippy, unit/integration tests | Required | Required | Required | Required |
| Linux/macOS/Windows build + E2E | Required for affected surfaces; required for merge to main under current policy | Required | Required | Required |
| Dependency/security audit | Required | Required | Required | Required |
| Binary object-format golden vectors | If touched | Required | Required | Required |
| Property-based DAG/merge tests | If touched | Required | Required | Required |
| Crash/fault-injection matrix | If state affected | Required | Required | Required |
| Reproducible Git migration fixtures | If adapter touched | Required | Required | Required |
| Remote authN/authZ and network fault tests | If server affected | N/A | Required | Required |
| 100-writer concurrency test | If concurrency affected | N/A | Required | Required |
| Backup + restore on supported platforms | If affected | Required | Required | Required |
| Format/upgrade/downgrade validation | If format affected | Required | Required | Required |
| Soak tests, integrity scrub, performance regression | For relevant changes | Baseline | Required | Required |
| Independent security + durability review | Relevant PR review | Review scheduled | Required for exposed server | Required and signed off |
| Signed release artifacts / SBOM / provenance | If packaging touched | Recommended | Required | Required |

A feature is **not complete** merely because a unit test exists or a specification describes it. It must be implemented, merged, and verified against the current source revision.

## 2. Mandatory no-data-loss scenarios

For every destructive workspace operation (switch, restore, reset, clean, merge, rebase, cherry-pick, checkout, migration):

- Existing tracked, untracked, staged, ignored, binary and conflict files are either preserved or modified only by the explicit requested operation.
- Abort and partial failure preserve a recoverable user-visible state.
- Repeating recovery is idempotent. No existing user data is silently replaced by ambiguous replay.
- Crash injection occurs before and after: intent journal, object write, flush, rename, directory sync, ref CAS and journal removal.
- Process termination and disk/power-loss claims are tested separately, on filesystem/backends for which guarantees are asserted.
- Failures are surfaced as clear errors with an operator repair/backup path.
- Concurrent writers produce one well-defined CAS winner and preserve the other writer's immutable changeset.
- Garbage collection never collects blobs referenced by reachable refs, active staging, conflict states, offline leases or in-progress uploads.

**Release blocker:** any reproducible case where acknowledged committed data is lost, an unauthorized path is overwritten, a hash mismatch is ignored or a ref points to missing/corrupt object data.

## 3. Format and compatibility certification

- Golden hash/test vectors across all claimed OS/CPU combinations.
- Roundtrips for blobs, trees, histories, merges, large files, file modes and symlinks (where claimed).
- Invalid and adversarial decode inputs rejected within bounded time/memory.
- Explicit format versioning and incompatible-feature detection.
- Migration from each supported earlier version with interruption recovery, restore and no-downgrade warnings.
- Git fixture matrix for supported SHA-1/SHA-256, signatures, tags, merges, LFS/submodules, unusual filenames and timestamps/timezones; unsupported features fail **explicitly**.
- Protocol version skew and CLI/SDK JSON schema compatibility tests.

## 4. Security certification

- Threat model and privilege map for local workspaces, server, caches and Git bridge.
- Token/OIDC/mTLS authorization, repository isolation and protected refs when a server is shipped.
- Object-ID guessing must not bypass repository permissions; cached responses follow the same authZ policy.
- Path traversal, symlink/hardlink and race tests; decompression bombs, corrupt blobs and malicious Git pack inputs.
- Rate limits, upload quotas, bounded payload streaming and secret-free operational logging.
- Vulnerability scanning of dependencies and CI supply chain; SBOM, signed release provenance and disclosure process.
- Independent review of storage integrity and authorization boundary before claiming production readiness.

## 5. Performance evidence (targets vs claims)

Publish fixture scripts, exact hardware/OS, repository size, history depth, active workspaces, data distributions and versions of all comparison systems.

Measure:
- `status`, staged commit, `diff`, `merge-base`, blame, sparse/full checkout and Git import/export.
- Native object reads/writes, bytes transferred, cache hit/miss, CPU, peak resident RAM, disk and GC overhead.
- p50/p95/p99 latency and throughput for 1, 10, 100, 500 concurrent agents.
- Correctness invariants during performance tests, not merely the number of operations per second.

**Proposed goals, not promises:** avoid full repository scans for unchanged small commits; 100 concurrent test clients with zero lost acknowledged commits; no unbounded memory with >64 MiB files. Numeric speedup claims require published evidence.

## 6. Branch and PR gate workflow

1. Inspect `main` and existing PRs/issues before creating a branch.
2. Scope each PR to one bounded behavior change, with explicit risks and backward compatibility note.
3. Provide tests for happy path, conflict/corruption and interruption if persistent state is touched.
4. Run formatting, Clippy, unit and integration tests locally or via CI; include platform matrix.
5. Validate required dependency audit, static checks and relevant security/concurrency tests.
6. Compare latest PR head SHA to the SHA on the successful CI run; stale checks **do not count**.
7. Inspect diff for unrelated changes, panic paths, accidental secrets, performance regressions and dropped errors.
8. Merge only when the correct head is mergeable and all required gates succeeded.
9. Verify `main` CI after merging and link the PR to parent/child issues.
10. Mark roadmap items `[x]` only after merged code + passing checks; update READMEs/status precisely.

Tests known to be flaky or unsupported should be fixed or explicitly documented as a release blocker, **not** silently skipped.

## 7. Release tier definitions

### Experimental alpha
- Safe only for disposable/test repositories.
- Format/CLI may change.
- Known worktree/platform limitations are visible on README and release notes.

### Local beta
- Reliable supported local VCS operations with a documented platform and file-type matrix.
- Stable-enough native format migration and backup/restore.
- Complete staging/restores/merges/history and integrity checks.
- No known data loss within declared crash model.
- Git migration conformance for a named supported feature subset.

### Collaborative beta
- Local beta requirements plus a secured native server.
- Atomic remote refs, resumable object transfer, offline divergence, authZ and 100 concurrent writers under fault injection.
- Self-hosting, monitoring, backup and upgrade path.

### Production v1.0
- All relevant gates above satisfied.
- Versioned stable on-disk and network compatibility policy.
- Signed artifacts, SBOM and published release notes.
- Independent storage correctness/security review and restore drills.
- Explicit list of supported features and unsupported limitations.
- Maintainer sign-off; a public release does not imply the project is comparable to global-scale Scion.

## 8. Evidence template for each milestone

```markdown
Milestone:
Exact Git SHA:
Supported OS/filesystems:
Feature matrix:
Cross-platform CI run URL and result:
Fault-injection/test-fixture run:
Security audit:
Git conformance result (if applicable):
Backup/restore exercise:
Performance datasets and report:
Known limitations / blockers:
Reviewer / release sign-off:
```

Do not delete a blocker simply because a new implementation approach is chosen. Close it with evidence that the relevant risk is addressed.
