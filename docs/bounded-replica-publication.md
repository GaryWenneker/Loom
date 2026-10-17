# Bounded parallel replica publication

Loom's replicated immutable-object backend uses all-ack publication: a write
succeeds only after every configured durable conditional-create backend has
accepted the same canonical envelope and returned those exact bytes on a live
read. Object IDs, envelopes and repository roots are unchanged by scheduling.

## Concurrency contract

`ReplicatedObjectBackend::new` selects at most four workers. Embedders can use
`with_workers(replicas, requested_workers)`; one forces the sequential control,
zero selects the default, and explicit requests are capped at eight and at the
replica count. Parallel execution starts only when at least two replicas exist.

The repository's outer immutable-object publisher deliberately still sees the
replicated backend's default one-write capability. Therefore only replicas of
one object overlap: object workers and replica workers cannot multiply into an
unbounded fanout. Every scoped worker is joined before success or error returns.
There is no detached work or implicit flush.

A failed replica can leave the same immutable object on successful peers. The
branch ref is not advanced, `exists` remains false until every replica contains
the object, and a retry is idempotent. The existing full object-closure check and
atomic compare-and-swap ref publication remain later gates. Reads continue to
validate hashes and object envelopes. This change does not parallelize or weaken
ref updates.

## Regression and measurement

The Rust regression test injects four observed durable backends and proves that
four requested replica writes overlap, all workers leave the active set, and the
object exists on every replica. The existing partial-failure test exercises
join-all failure handling and retry.

Build and run the checked benchmark on POSIX:

```sh
cargo build --release --locked -p loom-core --example replication_publication
python3 benchmarks/replica_writes.py \
  --binary target/release/examples/replication_publication \
  --files 100 1000 --workers 1 2 4 --replicas 4 --repeat 7 \
  --output replica-writes.json
```

Every sample uses a fresh workspace and four independent filesystem stores.
Only `Repository::commit` is timed internally. After that timing window, the
harness reopens the acknowledged revision from every individual store and runs
full `fsck` on each. The report retains p50/p95/p99 wall time, process CPU,
peak RSS, raw POSIX block-I/O counters, and logical/allocated replica bytes.
Whole-process resource metrics include fixture creation and verification;
worker order rotates and OS caches are not flushed.

There is no Git speed factor for this workload: Git has no directly comparable
single command that publishes one canonical commit to four independent durable
object stores under an all-ack contract. Network traffic, remote latency,
controlled cold cache, multi-agent contention, fault injection and power-loss
durability are also unmeasured. Results from the exact-head workflow are evidence
for this bounded local workload only, never a universal performance claim.
