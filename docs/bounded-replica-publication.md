# Bounded parallel replica publication

Loom's replicated immutable-object backend uses all-ack publication: a write
succeeds only after every configured durable conditional-create backend has
accepted the same canonical envelope and returned those exact bytes on a live
read. Object IDs, envelopes and repository roots are unchanged by scheduling.

## Concurrency contract

`ReplicatedObjectBackend::new` selects at most four publication workers and
serial live verification. Embedders can use
`with_workers(replicas, requested_workers)`; one forces both phases to run
serially, zero selects the default, and explicit requests apply to both phases,
capped at eight and the replica count. Parallel execution starts only when at
least two replicas exist. Worker counts are per call, not a global I/O budget.

The repository's outer immutable-object publisher deliberately still sees the
replicated backend's default one-write capability. Therefore only replicas of
one object overlap: object workers and replica workers cannot multiply into an
unbounded fanout. Every scoped worker is joined before success or error returns.
There is no detached work or implicit flush.

A failed replica can leave the same immutable object on successful peers. The
branch ref is not advanced, `exists` remains false until every replica contains
valid canonical bytes, and a retry is idempotent. The existing full object-closure check and
atomic compare-and-swap ref publication remain later gates. This change does not
parallelize or weaken ref updates.

`exists(id)` now reads **every replica live**, verifies the BLAKE3 ID and
canonical envelope, and acknowledges closure only when all copies validate.
Unverified existence flags and retained read sessions are never used. A missing
copy returns false if the other reads succeed; corruption, invalid encoding,
permission errors or read-worker panics fail closed. A read error takes
precedence over a missing-copy result. Each completed worker returns only a
boolean, releasing its payload before processing another replica. The
per-object payload limit and worker count do not imply a total RSS cap.

Ordinary `get(id)` still returns the first verified available copy; it is not an
all-replica closure acknowledgment and does not repair peers. There is no
automatic quorum downgrade, retry loop or read-repair policy. A new ref remains
blocked until the full required closure passes. Failed attempts may leave
unreachable immutable objects, while the previous ref and selected staging
snapshot remain available for a lossless retry. Same-key backend mutations
after verification are outside the immutable-provider contract.

## Regression and measurement

The Rust regression test injects four observed durable backends and proves that
four requested replica writes overlap, all workers leave the active set, and the
object exists on every replica. The existing partial-failure test exercises
join-all failure handling and retry. Additional tests reject a corrupt secondary
copy, lying existence flags and hash-matching malformed envelopes. Barrier and
channel tests prove read overlap, bounded fanout and join-on-error/panic. A
repository regression corrupts a historical secondary blob, rejects publication
without changing the branch or prepared bytes, then repairs and reopens the
acknowledged history independently from both copies after retry.

Build and run the checked benchmark on POSIX:

```sh
cargo build --release --locked -p loom-core --example replication_publication
python3 benchmarks/replica_writes.py \
  --binary target/release/examples/replication_publication \
  --files 100 1000 --workers 1 0 2 4 --replicas 4 --repeat 7 \
  --output replica-writes.json
```

Every sample uses a fresh workspace and four independent filesystem stores.
`Repository::commit` and a subsequent live verification of **all stored objects**
are timed separately. The latter excludes enumeration, includes unreachable
objects and uses warm stores immediately after commit; it is not a graph
traversal timing. Schema v2 records both distributions and the verified count.
After those timing windows, the harness reopens the acknowledged revision from every individual store and runs
full `fsck` on each. Every path and every fixture payload byte is checked, and
native tree roots must agree across replicas, worker counts and repeats. The report retains p50/p95/p99 wall time, process CPU,
peak RSS, raw POSIX block-I/O counters, and logical/allocated replica bytes.
Whole-process resource metrics include fixture creation and verification;
RSS may include a launcher floor and is not an isolated engine-memory result;
worker order rotates and OS caches are not flushed.

There is no Git speed factor for this workload: Git has no directly comparable
single command that publishes one canonical commit to four independent durable
object stores under an all-ack contract. Network traffic, remote latency,
controlled cold cache, multi-agent contention, fault injection and power-loss
durability are also unmeasured. Results from the exact-head workflow are evidence
for this bounded local workload only, never a universal performance claim.
