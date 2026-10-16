# Shared zero-cache directory reads — 2026-10-16

Grouped reads with `--cache-bytes 0` now share directory storage reads across paths. A nonzero budget selects parent grouping with verified cached ancestors. Layer routing is selected only when no objects can be retained: cheap cache hits can make the additional routing layer unnecessary. The results below retain every control and regression. A general 100× advantage is not established.

Measured [source ec8b9b8e](https://github.com/GaryWenneker/Loom/commit/ec8b9b8e78b8b4442a3d6b658cd70d2431f30728), tree `fc5fd86009374ce6fe1ceb0c43176a181c0aee74`. Baseline: main #81, `aff81704b2ebdbafac19b7665b53e6770ff462c4`. The follow-up adds evidence, a paired confirmation harness and its CI smoke check; it does not change Rust sources.

## Method

Both release binaries use Rust 1.99.1. The baseline was rebuilt on this host and produced the identical SHA-256 as the retained #81 binary. Host: Linux 6.18.44 x86-64, Intel Xeon Platinum 8573C, nine reported CPUs, eight-CPU cgroup quota, overlayfs (checked with `stat -f`). Git version is recorded in the raw files. Final benchmarks ran sequentially, without compilation or another benchmark in parallel.

The previous Loom initializes identical tracked bytes/history for all engines: 10,000 or 100,000 small binary files plus one noise file and 11 commits. Git uses packed objects and a changed-path commit graph. Sharded layout spreads files across 256 leaf directories; nested uses 1,024 leaves beneath 16 parents. A group contains 1,000 deterministic logical path requests with duplicates and 10% misses. All bytes, missing slots, order and framing are verified after the timed subprocess returns.

Every measured mode starts a new process. Startup, initial storage reads, input parsing, response framing and captured output remain in elapsed time. OS caches are not flushed; fully resident object pages are not verified. This is not a controlled cold-disk experiment. No read pack or off-timer query warmup is used. Git keeps its normal process cache when Loom caches are disabled.

Zero-cache comparisons use one group and five repeats, rotating all five modes. The default-cache regression controls use two rotating rounds of the same working set, eight repeats and all eight rotated modes. They also measure just the first group in a separate fresh process. This is 2,000 logical answers for the two-round control, not 2,000 distinct paths. It differs from the older 100-round working-set report; their ratios must not be compared as the same workload. The cache flag applies to query modes; independent CLI cat/files/status retain their normal semantics.

New binary SHA-256: `6fe0b40248b9fe502b8939904948295f764b14d239ea480a6e559d609bdaa8b2`; baseline: `e09bcee2121f153faa80717c6c4a25be51dcc8c4ed24ae2cca2c436384a6b4eb`; harness: `4b0109c4bb6ab33a8bc1f32ab354dce10f6305e96ae81fd3df8f25a8ca1905ec`.

## One group, Loom caches disabled

p50 milliseconds for all 1,000 logical paths, including process startup. Previous/new above one is an improvement over #81.

| Layout | Files | Previous Loom | New Loom | Packed Git | Previous/new | Git/new |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 10,000 | 38.41 | 29.95 | 187.95 | 1.28× | 6.28× |
| sharded | 100,000 | 60.88 | 68.95 | 221.74 | 0.88× | 3.22× |
| nested | 10,000 | 43.13 | 24.72 | 66.37 | 1.74× | 2.68× |

These are first-group results without retained Loom objects/path answers. They do not establish a 100× advantage over Git or a cold-storage guarantee.

## Paired native confirmation at 100,000 files

The five-sample sharded row above was 13% slower. A separate confirmation therefore alternates old/new order across 50 paired seeds on one new 100,000-file, 256-directory fixture with the same binary bytes and 11-commit recipe. Each sample starts a fresh zero-cache process and answers 1,000 paths. Every byte/slot is checked. Git and independent CLI/status timing are omitted in this confirmation; the original unfavorable controls remain above.

| Native engine | Samples | p50 ms | p95 ms | p99 ms |
| --- | ---: | ---: | ---: | ---: |
| Previous Loom | 50 | 75.64 | 143.44 | 461.13 |
| New Loom | 50 | 57.58 | 133.12 | 432.15 |

The ratio of p50 durations is 1.31×, or 23.9% lower elapsed time. This supports the zero-cache improvement for this confirmation workload; it does not erase the five-sample regression, establish causal explanations for all host variation, or supply a Git ratio. [Raw paired samples](../benchmarks/results/directory-native-pairs-20261016.json) retain individual ratios, binary hashes, host metadata and harness SHA-256 `a123e3f2f74f69d95e00adc817af596a4f6eda9d411c789d7e6f35929fc655d3`. Reproduce with [the paired native harness](../benchmarks/README.md#repeated-immutable-working-sets).

## First group with the default 64 MiB cache

Same 1,000 logical paths in fresh sessions. Ratios below one retain regressions; host variation and sample counts limit inference.

| Layout | Files | Previous Loom | New Loom | Packed Git | Previous/new | Git/new |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 10,000 | 23.78 | 38.06 | 160.51 | 0.62× | 4.22× |
| sharded | 100,000 | 85.19 | 111.13 | 330.07 | 0.77× | 2.97× |
| nested | 10,000 | 34.21 | 28.98 | 92.89 | 1.18× | 3.21× |

## Two rounds with the default cache

p50 milliseconds for both groups together, including the first group and startup. The second group rotates and reuses the same working set.

| Layout | Files | Previous Loom | New Loom | Packed Git | Previous/new | Git/new |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 10,000 | 27.83 | 25.21 | 321.50 | 1.10× | 12.75× |
| sharded | 100,000 | 69.03 | 68.76 | 722.53 | 1.00× | 10.51× |
| nested | 10,000 | 35.62 | 35.02 | 155.35 | 1.02× | 4.44× |

## Scalar and independent CLI controls

Scalar requests cover both rounds; each CLI column is one separate operation. Git/new below one means Loom is slower. All raw samples, p95/p99 and failed target flags remain available.

| Layout | Files | Scalar Git/new | Scalar previous/new | CLI cat Git/new | CLI files Git/new | Status Git/new |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 10,000 | 6.87× | 1.22× | 1.02× | 0.78× | 0.23× |
| sharded | 100,000 | 3.89× | 0.64× | 1.59× | 0.97× | 0.13× |
| nested | 10,000 | 2.33× | 0.87× | 0.70× | 0.67× | 0.24× |

Status still scans content and remains a substantial bottleneck. Timings are full-process durations, not per-response tail latency. Five/eight samples do not prove regression-free production behavior. CPU time, peak RSS, large files, cold disk, network and concurrent-client throughput were not measured. Earlier experimental traversal variants are not used in these tables.

## Algorithm, resources and integrity

The private zero-cache resolver routes already validated path suffixes by immutable directory ID and depth. Shared ancestors and identical directory DAGs are read once per depth. Each frontier contains at most 4,096 input-slot/byte-offset records; names borrow the captured input strings. Trees are processed serially in object-ID order and released before the next load, so only one active tree is needed outside the cache. There is no retained stack or whole layer of large tree buffers. Nonzero budgets use the existing parent-group strategy. Blob deduplication and bounded workers apply to both paths; serial providers skip CPU/cgroup discovery.

Each physical miss still validates hash, envelope and type. Syntax/depth validation precedes traversal. Duplicate content counts repeatedly toward the returned-byte budget. Path answers are published only after the whole group succeeds. Verified cached objects never certify live existence, publication, recovery or fsck. Cache accounting is not an RSS limit: frontier metadata, active read buffers and consumer-held Arcs are additional.

199 local tests pass with strict fmt/Clippy. Added cases cover shared ancestors/DAG aliases, Unicode suffix offsets, zero/tiny/default budgets, duplicate byte limits, corruption, no partial path answers, one tree referenced at different depths, invalid directory types and empty child trees. Grouped reads also exercise the 256-component path boundary. Existing binary protocol, SQLite, read-pack, concurrent-reader and live-integrity suites pass.

[Reproduce with cache budgets](../benchmarks/README.md#repeated-immutable-working-sets), [protocol/resource contract](snapshot-query-protocol.md#storage-and-integrity). Raw reports: [zero-cache nested](../benchmarks/results/directory-zero-cache-nested-20261016.json), [zero-cache sharded](../benchmarks/results/directory-zero-cache-sharded-20261016.json), [default-cache nested](../benchmarks/results/directory-cache-control-nested-20261016.json), [default-cache sharded](../benchmarks/results/directory-cache-control-sharded-20261016.json). The separate [100-round working-set evidence](performance-working-set-20261016.md) concerns the previous implementation/run; it is not a new first-read claim.
