# Shared publication-memory evidence (2026-10-20)

## Change under test

Every `ObjectPublisher` previously bounded its own window to 128 objects or
8 MiB, but cloned publishers had no shared byte bound. This change adds one
shared reservation to `ObjectStore`. Accepted canonical envelopes and their
maximum fresh live-readback allocation are charged before entering a publisher.
A full budget blocks another producer until a verified window, failure or drop
returns its reservation. The default still admits one maximum-size v1 object.

The reservation does not change canonical IDs, window boundaries, SQLite
transactions, WAL/FULL durability, conditional creation, replica obligations,
fresh readback, closure verification or ref CAS. It also does not claim to cap
caller-owned input, SQLite/page caches, stacks, allocator overhead or OS cache.

## Correctness evidence

The focused regression uses two concurrent publishers cloned from one store and
a budget that admits exactly one object's canonical bytes plus readback. It
proves the second producer remains blocked until the first unfinished publisher
is dropped, and then completes. A separate regression rejects a reservation that
cannot cover both canonical bytes and live readback. Existing publication,
rollback, retry, replica and repository tests remain required.

## Performance method

Use `benchmarks/sqlite_batch_publication.py` with separate candidate and exact
previous-main binaries. Both receive identical repository roots, source bytes,
directory arguments and explicit worker requests. The workflow times complete
durable initial and incremental commits, then validates roots, queries and
`fsck`. Git `add` plus `commit` remains an external baseline with a different
storage contract.

The normal single-process CLI workflow owns one publisher, so it is primarily a
no-regression control for this change. Its RSS floor cannot prove the concurrent
shared cap. The deterministic Rust concurrency regression is the direct bound
proof. A future embedded multi-workspace harness must measure peak RSS and queue
wait under 1/10/100/500 simultaneous publishers; the existing
`shared_object_stress.py` starts separate CLI processes and therefore cannot
exercise a process-local shared reservation.

Record p50/p95/p99 wall and CPU time, peak RSS, block I/O, logical/allocated
database bytes, exact argv, binary/tree/harness hashes and adverse samples.
Cold-cache, fsync counts, WAL backlog, network, crash/power-loss recovery and
cross-process/remote quotas remain explicitly unmeasured unless a later artifact
provides them. No throughput or Git speedup follows from adding backpressure.

## Exact implementation-head result

GitHub Actions run `39396533305` compared candidate
`4dd3207139023dfd5563e50dfda0ed7d54029848` with previous main
`34a9f60bdbf678c4666e0fb17423167213ef079f`. Candidate tree was
`d96a9485cae4d9324e54ac71408ee370c5ddfe53`; candidate and baseline binary
SHA-256 values were respectively
`72cfa8bced6a60f5458f4311eb0dad091ad36590c156751e616115d337fae2f3` and
`10b1e37024759e0a4fd2a2b67ab661025d5967eb83950382d78c3a4ea42b6e8d`.
Artifact `12253255334` has digest
`sha256:648879077819a90e21a4267a23dda43636c9be92e34e260edacfb3a22b0734ca`.

| Files / directories / workers | Initial candidate p50/p95/p99 | Previous p50 | Incremental candidate p50/p95/p99 | Previous p50 | Git initial / incremental p50 |
|---|---:|---:|---:|---:|---:|
| 32 / 32 / auto | 182.02 / 460.31 / 475.04 ms | 220.96 ms | 76.70 / 183.00 / 195.95 ms | 87.26 ms | 11.97 / 7.37 ms |
| 256 / 32 / auto | 400.18 / 882.23 / 1017.11 ms | 301.85 ms | 181.84 / 404.43 / 439.38 ms | 169.66 ms | 38.02 / 13.25 ms |
| 256 / 32 / one | 280.09 / 765.30 / 808.73 ms | 255.68 ms | 179.08 / 552.57 / 625.97 ms | 149.46 ms | 38.25 / 13.72 ms |
| 2048 / 64 / auto | 1134.46 / 1310.72 / 1332.12 ms | 1033.36 ms | 1205.10 / 1330.41 / 1344.66 ms | 1007.72 ms | 228.64 / 31.69 ms |

The result is adverse or unstable, not a speedup result. Candidate p50 wall time
was 9.8% to 32.6% slower in six of eight commit comparisons and 17.7%/12.1%
faster only in the smallest pair. Very large wall p95/p99 values with almost
flat CPU show severe shared-host waiting. Initial CPU p50 ranged from about 1.9%
worse to 1.7% better than previous Loom, and paired peak-RSS floors were equal
inside every scenario. Metadata footprints and canonical roots were identical;
input block counters were zero. Git remained faster for every measured complete
commit. These single-publisher runs neither exercise nor quantify the new shared
cap, so no performance factor is attributed to it and the adverse samples are
retained.
