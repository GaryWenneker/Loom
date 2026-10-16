# Repeated working-set performance — 2026-10-16

The grouped working-set p50 reaches 100× packed Git in 2 of these three controls. This is a repeated immutable-path workload; it is not a general 100× guarantee. Fresh-session first groups, scalar requests and status are reported separately.

Measured [source 6a487d2f](https://github.com/GaryWenneker/Loom/commit/6a487d2f0874a4221d37f3c80a4ffdfada3b44d3), tree `c43b5501a45e67804a6476648b8dd02069b50021`. Baseline is main #80, `8225d863ff6504c5628103b15aefd4efd079ed99`. The evidence follow-up changes only reports.

## Workload and method

Eight repeats per scenario and eight rotating batch modes for a complete engine-order cycle. Each sequence is 100 rounds of 1,000 logical path requests, for 100,000 answers. Each round rotates the same working set, with duplicates and 10% misses. This is not 100,000 distinct paths. The baseline initializes identical fixture bytes/history for both engines: N small binary fixture files, one noise file and 11 commits. Sharded uses 256 leaf directories; nested uses 1,024 leaf directories under another 16-way level.

Each measured mode starts one new process and keeps it for its entire sequence. Both old/new Loom use native `cat_many`; Git uses persistent packed `cat-file --batch` path queries. Every byte, missing slot, order and frame is verified after the timed subprocess returns. Startup, first-group object I/O, input JSON parsing, output framing and capture remain in elapsed time. No read pack or off-timer query warmup is used. The OS cache is warm; engine caches warm naturally during each sequence. Git gc and changed-path commit-graph preparation are outside query timing.

Both Loom release binaries use Rust 1.99.1 and the same host: Linux x86-64 container, Intel Xeon Platinum 8573C, nine reported CPUs, eight-CPU cgroup quota, overlay filesystem. Git: git version 2.51.1. Default 64 MiB accounted snapshot-cache budget for both Loom versions. Hardware/load variation and eight samples limit inference; p95/p99 describe full-sequence durations, not per-response tail latency. Peak RSS, CPU time, cold storage, network and multiple clients were not measured.

Binary SHA-256: `e09bcee2121f153faa80717c6c4a25be51dcc8c4ed24ae2cca2c436384a6b4eb`. Baseline and harness hashes are retained in both raw files. Prior development probes are not used in these tables.

## Full-sequence grouped p50 (milliseconds)

| Layout | Files | Previous Loom | New Loom | Packed Git | New vs previous | Git/new | 100× p50 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| sharded | 10,000 | 146.75 | 40.23 | 8765.75 | 3.65× | 217.88× | yes |
| sharded | 100,000 | 167.30 | 55.66 | 15215.27 | 3.01× | 273.37× | yes |
| nested | 10,000 | 232.63 | 56.18 | 4234.63 | 4.14× | 75.38× | no |

These are totals for 100 groups. They do not establish a p50 latency for an individual round. The native-frame before/after comparison uses the same request/response semantics. Git has different framing while returning the same logical bytes/missing answers. No per-file Git launches are used.

## First group in a fresh session (p50 milliseconds)

These additional modes query just the first 1,000 paths in the same fixture, including new process/session startup. OS caches remain warm. They do not reuse the preceding engine process.

| Layout | Files | Previous Loom | New Loom | Packed Git | New vs previous | Git/new |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 10,000 | 17.52 | 17.84 | 102.71 | 0.98× | 5.76× |
| sharded | 100,000 | 26.61 | 28.07 | 161.34 | 0.95× | 5.75× |
| nested | 10,000 | 28.08 | 38.47 | 68.87 | 0.73× | 1.79× |

The 100× result must not be applied to these first-group reads. Query startup and uncached tree/blob work remain. Working sets that change continuously, exceed the cache, use zero cache or span new roots need separate performance evidence.

## Retained scalar and single-command controls

Scalar lines below cover the same full 100-round sequence. CLI cat/files/status each measure one command per sample. Git/new below one means Loom is slower.

| Layout | Files | Scalar Git/new | Previous/new scalar | CLI cat Git/new | CLI files Git/new | Status Git/new |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 10,000 | 31.97× | 1.46× | 0.82× | 1.08× | 0.22× |
| sharded | 100,000 | 48.73× | 1.54× | 0.88× | 1.17× | 0.16× |
| nested | 10,000 | 14.03× | 3.24× | 2.09× | 0.80× | 0.36× |

Losses and failed target flags are retained. Status still scans content and is much slower than Git. Scalar clients retain per-request JSON/framing/flush work and do not obtain the grouped-mode ratio. These samples do not demonstrate a universal regression-free or production latency guarantee.

## Change, resources and correctness

A Snapshot memoizes exact positive/negative path answers under its pinned immutable root. Only previously validated keys and verified answers can hit. Generic `AsRef<str>` inputs are captured once before validation/lookup/insertion, including implementations that choose another view on later calls. New misses still validate syntax/nesting and hash/type/length through the normal reader. Worker CPU/cgroup discovery is deferred until enough physical object I/O needs parallelism. No object format, mutable-ref, durability or workspace semantics change.

Path and object entries share one FIFO and the existing budget. Accounting includes both owned path-key strings, estimated bookkeeping, and conservatively counts payload per path even if Arcs share bytes. Zero budget disables both. `path_cache_hits`/`path_cache_entries` expose reuse; total cache counters now include path entries. This can displace object entries and affect workloads with churn. Returned Arcs, temporary vectors, I/O buffers and OS caches remain outside that accounted budget; no RSS cap or reduction is claimed. Cached answers never certify live object presence, publication, recovery or fsck.

197 local tests pass plus strict fmt/Clippy. Added coverage checks partial/complete cache hits, missing/directory slots, cached duplicate-byte limits, later revisions, mixed object/path FIFO eviction under tiny and zero budgets, all-or-error path-answer insertion after corruption, and a safe Rust input-view switch that must not poison another path. Existing binary framing, lock independence, SQLite, corruption and provider suites remain intact.

[Reproduction](../benchmarks/README.md#repeated-immutable-working-sets), [cache/protocol contract](snapshot-query-protocol.md#storage-and-integrity), [raw sharded](../benchmarks/results/working-set-sharded-20261016.json), [raw nested](../benchmarks/results/working-set-nested-20261016.json). The broader first-read/write/status/multi-agent and production goals remain open.
