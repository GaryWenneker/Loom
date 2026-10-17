# Bounded full-content worktree scans

Legacy commit-all and status now scan changed content through bounded windows instead of retaining the complete worktree payload. Each file is still read and hashed in full; no mtime/stat cache decides content identity.

## Scheduling and bounds

A read window contains at most 1,024 regular files and 8 MiB of discovered payload lengths. One larger file (up to the existing 64 MiB payload cap) runs alone and serially. Every read is limited to its discovered length plus one byte. Changed lengths fail the operation; canonical headers and BLAKE3 IDs remain byte-for-byte compatible. Files are read directly into their canonical buffer, avoiding a second payload copy.

`WorktreeScanOptions { workers: 0 }` selects up to four workers. Explicit requests are capped at eight and available CPUs. Windows with fewer than 32 files run serially. CLI examples:

```sh
loom status --scan-workers 1
loom commit -m "update" --scan-workers 4
```

Worker limits are per scan, not a global quota across processes. All scoped readers join before a window is consumed or an error returns. Publication uses the separate existing bounded ObjectPublisher, conditional create, sync and verified live readback. Pending object publication finishes before tree/changeset/ref publication. A scan or provider failure can leave immutable orphans, but cannot acknowledge a new branch ref. Retry remains idempotent. Explicit staged commits retain their change-sized path and do not start a full scan.

The window limit is **not a total RSS cap**. Canonical headers, one-byte overrun probes, reader scratch/runtime allocations, the separately bounded publication window, path and deletion bookkeeping, decoded history and changeset/tree construction are additional. A large serial file is an explicit exception. File contents are no longer all retained, but metadata still scales with tracked paths and changed paths. Diff, materialization, recovery and merge paths keep their existing scans; this change does not bound their full payload memory.

## Filesystem consistency

The normal Loom worktree lock still serializes Loom mutations. External editors can modify files during a scan; this is not an atomic filesystem snapshot. Discovered/opened/read lengths must agree, but same-length concurrent writes can still race. Unix opens reject leaf symlinks and use nonblocking mode before checking regular-file type. Parent directories are not traversed with anchored handles. The portable fallback checks leaf type before opening but does not establish race-free reparse-point confinement. No new sandbox guarantee is claimed.

Existing ignore semantics remain: rules only suppress untracked paths, tracked files remain visible, and .loomignore never hides itself. Unsupported file types/path identities fail explicitly. Sorted changes preserve deterministic native tree identity across worker settings. Status compares IDs without retaining old payload bytes; full integrity certification remains fsck's responsibility.

## Validation

The regression suite covers serial/parallel native roots and change kinds across multiple count windows, duplicate content, binary/empty/large files, ignored tracked changes, post-stage edits, capped workers, length drift, failed read windows, unsupported symlinks and oversized-input ref safety/retry. Existing provider fault injection checks a failure after a published immutable window leaves refs/root mappings unchanged and drains all writers.

The checked publication benchmark measures initial and eight-file commits, changed and clean status, with Git as an independent external baseline. Fixture preparation, full path/payload/framing checks and fsck remain outside timing. Status checks are validated exactly; all unfavorable results and resource-accounting limits must be retained. Full scans still visit every directory/file and replay v1 history; change-sized discovery, filesystem event journals, remote scheduling and #26/#13 release gates remain open.

[Measured results and limitations](performance-worktree-scans-20261017.md) include raw complete-workflow and scan-worker controls.
