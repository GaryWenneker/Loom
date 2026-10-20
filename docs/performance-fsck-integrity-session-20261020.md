# SQLite fsck integrity session — 20 October 2026

This change gives `fsck` its own operation-scoped immutable reader. The SQLite
implementation opens one fresh read-only connection when the integrity check
starts and reuses only its parsed object `SELECT`. Every returned envelope is
still independently bounded, decoded and checked against its requested BLAKE3
object ID in core.

The session is deliberately separate from the ordinary immutable query reader.
It is never used by commits, history, file listings, recovery, publication
readback or closure acknowledgement. This avoids the cross-command contention
and latency regressions measured in rejected PR #118.

## Correctness contract

- The database must already exist and remain the same regular file for the
  complete scan. Removal or replacement fails closed before or after a read.
- SQLite reads remain read-only and `query_only`; write durability remains WAL
  plus `synchronous=FULL`.
- The core rechecks canonical envelope structure and object hash for every
  value. A provider session is not trusted merely because it returned bytes.
- Providers that do not opt in retain the existing scalar, live-read fallback.
- No object, ref, staging row, index or format marker is changed by `fsck`.

## Measurement protocol

The existing checked SQLite workflow compares this candidate with the exact PR
base in separate release targets. It creates fresh repositories with identical
bytes, directory arguments and explicit scan-worker requests, then times full
initial/incremental commits plus `fsck`, history, listing and cache-disabled byte
queries. It records p50/p95/p99 wall and CPU time, peak RSS, block-I/O counters,
actual argv, binary/harness/tree hashes and logical/allocated storage. Git is
retained as a diagnostic baseline; its `fsck` does not implement Loom's object,
receipt and portable-closure contract.

CI results and unfavorable cases will be added to the pull request before any
merge decision. Controlled cold-cache data, fsync counts, network traffic,
cache-hit rates, crash/power-loss recovery, soak and 1/10/100/500-agent evidence
remain unmeasured. No 100x or release-readiness claim follows from this change.
