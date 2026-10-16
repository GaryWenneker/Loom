# Architecture and performance review — 2026-10-16

Baseline: `edaf3feaf4f9acfe8be1aa62044149a00698dcc8`. Source inspection and
local Linux measurements; no independent production certification.

| Area | Finding | Change / remaining work |
| --- | --- | --- |
| Query lifecycle | Ordinary discovery can take the workspace lock, run migrations/recovery and verify complete external history before serving a single immutable read. | Separate read-only snapshot opener; no mutable workspace capability or recovery. Normal publication/discovery remains strict. |
| Tree access | Historical scoped listings can build a full SQL snapshot; repeated reads decode the same wide tree. | Direct native-prefix lookup, revision-pinned decoded tree/blob cache and a persistent binary-safe query protocol. Full listings still have materialization costs. |
| SQLite objects | Each object read opens/configures another database connection. | Optional object read session retains one query-only connection. Write acknowledgements and closure checks continue to open/check live storage. |
| Memory | An unbounded cache could hide an operational regression. | Explicit 0–256 MiB accounted cache budget; input/page/nesting/traversal bounds. RSS and caller-retained values are not bounded by that budget. |
| Workspaces | Read-only agents should not queue behind a workspace writer. | Lock-independence and untouched-recovery-journal tests. Writable agents still need independent workspaces; shared writable worktree coordination remains future work. |
| Status | Full content scans lose to Git's mature index/stat path. | Explicit remaining bottleneck. A future stat cache must detect same-size/mtime changes, replacements, racy timestamps and platform differences before being trusted. |
| CI | `cargo fmt` rewrites source and succeeds; Clippy warnings do not fail; dependency resolution was floating. | CI now checks formatting, denies warnings, uses committed Cargo.lock for builds/audits, and checks the declared Rust 1.85 minimum. |
| Hosting gates | Repository rulesets API returned an empty list. Integration lacks permission to read branch protection (403). | Exact-head Linux/macOS/Windows/security checks remain mandatory before our merges. This observation does not prove branch protection is absent. No hosting settings were changed. |

## Performance acceptance

The requested 100× Git speedup is a **per-operation target**, not a blanket claim.
The checked-in comparison harness exercises the same bytes and validates every
answer against Git, including Git's persistent batch mode. Wide-directory batch
reads can exceed the target. Single CLI calls, status, writes, merges, sparse
checkout, network replication and production concurrency need their own evidence.
Do not average these operations into one advertised speedup.

The baseline on 10,000 tracked files had clean status about 5× slower than Git.
Improving immutable reads does not resolve that cost. Published raw results must
retain failed target flags as well as successful ones. See
[benchmark reproduction](../benchmarks/README.md) and
[query capability boundaries](snapshot-query-protocol.md).

## Next architectural priorities

1. Trustworthy incremental worktree metadata with adversarial timestamp and
   replacement checks; measure status and staged commit independently.
2. Portable v2 blob metadata/streaming and checked format migration; current
   object and symlink-race limitations still apply.
3. Path-scoped materialization and independent writable workspace coordination.
4. Server-authoritative refs, permissions, partition/retry semantics and real
   multi-agent fault workloads; local SQLite stress is not proof of these.
