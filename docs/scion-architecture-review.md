# Scion benchmark: architecture gap assessment

**Target:** Anthropic's Scion source-control architecture, not Google's agent-orchestration product with the same name.  
**Evidence:** Anthropic engineering article, 13 October 2026: https://www.anthropic.com/engineering/redesigning-source-control  
**Loom revision reviewed:** `main` at `699418b7` and the documentation review branch.  
**Verdict:** Loom is a reasonable native local alpha, not yet an architecture demonstrated to compete with Scion at agent/monorepo scale.

| Dimension | Loom today | Scion's reported design | Loom gap |
|---|---|---|---|
| Authoritative state | Local SQLite + immutable loose objects | Server-authoritative changesets; Spanner refs | Native server and multi-node authority |
| Commit complexity | First-parent replay; initial add/commit rescan the full worktree | O(changed paths) server-side commit goal | Native trees and derived path indexes remain necessary |
| History/blame | First-parent log; graph scans | Derived ancestry, path and blame structures | Indexed history and merge bases |
| Checkout | Full local files, partial crash journal | Sparse/virtual demand materialization | Safe sparse workspace protocol |
| Cache topology | Per-repository loose objects | Shared daemon, regional caches, object services | Tiered content caches with leases |
| Concurrency | Per-workspace file lock and local CAS | Lock-free creation, ref CAS, server scheduling | Concurrent changesets and queue semantics |
| Remote/offline | Offline local alpha, no remote | Always-online client with server-side operations | Secure sync; preserve Loom's offline advantage |
| Large data | 64 MiB per native object | Huge monorepo, object fetch by identity | Chunking, streaming, storage scale |
| Durability | Partial process-crash replay | Large production deployment; internal fuzzing | Fault matrix, backup/restore, audit |
| Performance evidence | Cross-platform functional CI only | Vendor-reported large-scale operational gains | Independent reproducible benchmark fixtures |

**Development progress:** selected-file staging no longer scans the whole worktree, and a derived SQLite branch path index now supports incremental staged commits. Full-worktree status, arbitrary revision reconstruction, scalable derived blame/ancestry, native tree format and server-side commits remain incomplete. No speedup is claimed without reproducible measurements.

**Incremental read improvement:** the current branch path index can now answer direct branch-tip file reads in one snapshot-consistent SQLite query, without replaying the first-parent DAG. When the derived index is absent or its revision marker no longer matches the authoritative ref, Loom falls back to immutable history. Direct native prefix-scoped branch file listings can also use the index without checkout. This still does not implement general revision queries, ancestry, blame, filesystem virtualization or sparse remote retrieval.

## Architectural recommendations

1. **Fix integrity before new services.** Object creation must never overwrite an existing immutable path. Harden filesystem traversal, staged read boundaries and process/power-loss fault injection.
2. **Build native tree and index contracts.** Incremental status/commit complexity must scale with changed files. Persist changesets independently and maintain rebuildable derived path/ancestry/blame indexes.
3. **Separate the core domains.** Use cohesive objects/format, storage, refs, workspaces, staging, graph, merge and sync modules behind versioned interfaces. Avoid rewriting solely for cosmetic purity.
4. **Support two deployment modes using one format.** Local-first embedded offline mode and a server-authoritative mode, with replayable idempotent sync, CAS refs and explicit divergence.
5. **Implement sparse, on-demand workspaces.** Direct path and graph RPCs, shared node object caches, paginated queries, streaming manifests and bounded client storage.
6. **Engineer concurrent agent workflows.** Isolated mutable workspaces, immutable content sharing, lease-backed GC safety and a scalable landing queue. Avoid repo-global locks except for the smallest local safety boundaries.
7. **Prove results.** Run 10k/100k/1m path fixtures, 1/10/100/500 writers, crash matrices and p50/p95/p99 with raw scripts and hardware data. Do not quote Scion's reported speedups as Loom results.

## A viable differentiation

Scion optimizes for clients that are **always online**. Loom could offer one portable native content model for both full offline development and scalable remote operation. This is a credible product hypothesis only once migration, synchronization, durability and performance are proven.

## Release decision

**No-go for Scion-scale competitiveness today.** Continue as an experimental source-control engine. Reassess after secure remote services, indexed history, sparse workloads, object-cache benchmarks and safety certification are implemented.

Anthropic's internals and reported performance are not independently verified here; this comparison evaluates the public architecture description against the actual Loom source.
