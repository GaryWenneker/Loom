# Senior architecture review: capability-first release gates

**Reviewed:** 2026-10-15. **Baseline:** main after PR #56. **Verdict:** no-go for Scion-scale competitiveness. This is a source review, not a claim of independent production testing.

## Evidence from implementation

| Severity | Source | Finding | Required corrective work |
| --- | --- | --- | --- |
| Critical | `crates/loom-core/src/store.rs` | `checked_parent` verifies paths but documents remaining concurrent symlink/directory substitution races. | Handle-relative, no-follow file access or a clearly documented single-trust-user threat model; fault tests before network exposure. |
| Critical | `crates/loom-core/src/format.rs` | BLOB and CHANGESET still use v1 while TREE uses v2; 64 MiB object cap, no blob manifest. | Canonical v2 schemas, domain separation, chunked streaming, independent golden vectors, crash-safe migration. |
| High | `crates/loom-core/src/tree.rs` | Native path lookup and copy-on-write trees are internal primitives; file metadata is limited to a reserved executable flag. | Expose safe sparse workspace API, metadata-aware paths, bounded cache and remote fetch. |
| High | `docs/scion-architecture-review.md` | No complete ancestry/blame index or server-authoritative secure protocol. | Rebuildable derived indexes, remote CAS, idempotent object uploads, authz, partitions and retries. |
| High | `docs/roadmap.md` | Agent-scale reliability and benchmark acceptance are not demonstrated. | Reproducible 1/10/100/500-agent fault workloads and 10k/100k/1M-path benchmark artifacts. |

## Corrected execution order

1. **P0 correctness first:** storage publication hardening, no-follow path boundaries, complete reachable object closure and durable ref transitions; prove crash and corruption behavior.
2. **P0 portable v2 format:** typed object schemas, metadata, symlink policy, chunking, stable IDs and independent vectors; build reversible verified v1 migration.
3. **P1 native history indexes:** generic rebuildable ancestry, path and blame indexes with explicit invalidation and memory bounds; benchmark path-local queries.
4. **P1 user-visible sparse workflows:** path-scoped reads, sparse checkout/materialization, bounded local caches and safe user-edit preservation. Internal `lookup_file` alone is not a delivered feature.
5. **P1 remote authority:** versioned secure protocol, streaming, object closure, idempotent retries, per-directory authorization and durable compare-and-swap refs.
6. **P2 multi-agent coordination:** isolated workspaces, append-only operations, no global lock for independent changeset creation, merge queue and conflict reporting.
7. **P2 release proof:** fault injection, backup/restore drills, cross-platform compatibility and reproducible performance comparisons with raw artifacts.

## Mandatory review policy

- Do not mark a roadmap item complete until the capability is merged, externally reachable where required, and demonstrated with meaningful tests.
- Prefer one cohesive feature PR over dozens of isolated test-only PRs; tests accompany behavior, not substitute for it.
- Each feature PR must describe API behavior, failure modes, compatibility, rollback, threat model and performance implications.
- Before merge require exact-head Linux, macOS, Windows and security checks; after merge validate the resulting main commit.
- Do not claim Scion parity or superiority until issue #26's measurable release gates are independently reproducible.
- Keep the product standalone; no coupling to unrelated products.
