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
