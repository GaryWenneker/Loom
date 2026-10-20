# Shared snapshot-cache evidence (2026-10-20)

## Change under test

Snapshot caches previously enforced only their individual zero-to-256 MiB
budgets. Multiple long-lived snapshots cloned from one repository could
therefore multiply retained verified trees, blobs and exact path answers without
a store-wide bound. The store now owns one 256 MiB retained-cache reservation
shared by those snapshots.

Admission is deliberately nonblocking. Waiting would allow one long-lived
snapshot to deadlock another query. A cache first evicts its own FIFO entries;
if other snapshots still occupy the shared ceiling, Loom returns the verified
answer without retaining the new entry. Drop and eviction return reservations.
`cache_admission_rejections` makes this pressure observable.

This does not alter canonical bytes, IDs, object reads, snapshot consistency,
publication, WAL/FULL durability, replica verification, closure checks or ref
CAS. It does not cover separately opened stores or processes, caller-owned Arcs,
read-pack indexes, temporary read buffers, allocator overhead or OS page cache,
and therefore is not a total RSS claim.

## Correctness evidence

The focused regression creates three snapshots over one cloned store with a
shared budget that fits exactly one decoded root. The first retains the root;
the second remains readable but records a rejected cache admission; dropping the
first lets the third retain the same root. Existing snapshot, grouped-query,
sparse-export and integrity regressions remain required on every platform.

## Performance method

The existing checked sparse-export workflow compares the exact candidate and
previous-main binaries with identical source bytes, roots, file counts and
worker requests. It times the complete verified export and records wall time,
CPU, peak RSS, block I/O, output footprint, binary/harness/tree hashes and Git's
separate diagnostic export. A CLI invocation creates only one snapshot, so this
is a no-regression control rather than proof of the shared ceiling.

An initial candidate run incorrectly sent previous Loom through a decomposed
`files`/`query` compatibility client even though previous main supports native
`sparse-export`. That run is diagnostic only and must not be presented as a
version speedup. The corrected harness starts both binaries with the same native
command shape and identical directory selections. `sparse-export` has no worker
control, so neither side receives a hidden or unequal worker request. Genuine
baseline failures are not masked as unsupported.

The deterministic in-process regression is the direct bound proof. A future
embedded benchmark must retain 1/10/100/500 simultaneous snapshots, drive hot,
cold and churned working sets, and record aggregate cache bytes, rejected
admissions, peak RSS, queue time and cache hit rate. Network, remote caches,
power loss and crash/soak behavior also remain unmeasured. No speedup follows
from rejecting cache admission under memory pressure.
