# Specialist engineering review

**Baseline:** Loom 0.1 alpha, reviewed on 15 October 2026. These are source-inspection findings, not completed fixes or audited production guarantees.

| Severity | Finding | Code area | Required work |
|---|---|---|---|
| Critical | Object publication uses rename after a check. Competing processes can overwrite the same immutable object path. | `store.rs::put` | Atomic create-if-absent, verify competing object and test races |
| Critical | Object reads follow symlinks and trust path components. | `store.rs::get` | No-follow traversal with platform-specific tests |
| High | Worktree materialization and the DB are separate durability domains; partial recovery is not proof against power loss or path races. | `repository.rs::materialize` | Fault injection and safe recovery model |
| High | Full worktree scans on add, commit, status, diff and switching; unrelated large or unsupported files can block useful operations. | `repository.rs` | Incremental staging and indexed tree queries |
| High | History state requires first-parent replay; merge-base handling does not cover every multi-base DAG. | `repository.rs` | Rebuildable checkpoints, robust ancestry indices and merge-base semantics |
| High | Native v1 lacks full metadata and files larger than 64 MiB. | `format.rs` | Native v2 portable trees, manifests, tested migration |
| High | CI does not assert clean formatting and generates a fresh dependency lock for its audit. | `ci.yml` | `cargo fmt --check`, committed lockfile and locked builds |
| Medium | Repository code combines storage, graph, merges, workspace and ref coordination. | `repository.rs` | Modular domain boundaries after contracts stabilize |
| Medium | CLI has ad hoc argument parsing and no versioned machine-readable output. | `loom-cli` | Typed parser, JSON schema and stable diagnostic codes |

## Strengths

Immutable content-addressed objects, canonical envelopes, transactional ref compare-and-swap, native history separate from interoperability and a limited process-crash intent journal are sound starting points.

## Release rule

Do not represent Loom as equivalent to a production VCS or Scion until storage fault tests, data integrity, large-repo benchmarks, native networking, sparse/virtual workspaces and concurrency experiments have passed. A green build alone is insufficient.

See [architecture](architecture.md), [roadmap](roadmap.md), [release gates](release-gates.md) and [capability inventory](git-capability-parity.md).
