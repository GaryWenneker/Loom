# Bounded immutable object publication — 2026-10-16

Legacy commit-all now publishes changed blobs through bounded provider-controlled workers. These measurements compare complete initial-commit and eight-file-change workflows. They do not establish a universal 100× advantage or Scion-scale release readiness. All incremental losses against Git remain visible.

Measured [source 9508e2f6](https://github.com/GaryWenneker/Loom/commit/9508e2f66e6d2830db87b3e9c6e5b2298103d0d5), tree `a54f1bc3ce883ed8fb86f38cf199d42d8379fcc6`. Baseline is main #82, `5934d95904fe6093dfa58e4b5d9c10aa41005ad2`. The follow-up adds these reports and clarifies the RSS measurement; production Rust sources remain identical.

## Method

Both binaries are release builds with Rust 1.99.1, built/run on this Linux x86-64 AMD EPYC 9V74 host (nine reported CPUs, eight-CPU cgroup quota, overlay filesystem). The baseline rebuild reproduced its retained hash. This is a different host from the earlier Intel working-set reports; timings must not be compared across them. Git version, platform and CPU model are in the raw reports. Final benchmarks ran sequentially without compilation or a second benchmark in parallel.

Each engine/sample receives a fresh repository with identical unique 256-byte binary files. Sharded layout uses 256 leaf directories; nested uses 1,024 leaves under 16 parents. The initial workflow tracks and commits every file. The second modifies eight files distributed through the same fixture. Source preparation is outside timing. Both Loom versions use native commit-all; Git uses **add plus commit**, with both timed commands summed. Native workflow startup is one process, Git workflow startup two; this reflects those CLI workflows, not a comparison with an already staged Git commit. Global Git configuration and inherited provider selection are removed.

After each timed workflow, the harness checks every path, every payload byte, missing/extra slots and framing, and runs full fsck. Old/new native tree roots must match after each commit. These checks are outside timing, can warm later access, and must not be interpreted as cold storage. Git/native fixtures have the same logical contents and history depth; their formats, compression and metadata differ.

The 1k/10k controls have six repeats, rotating all three engines for two complete order cycles. The 100k control has three repeats, one cycle, so its percentiles are particularly weak. Raw samples plus p50/p95/p99 remain available; these describe complete workflows rather than individual object latency or production tail SLOs. OS caches are not flushed and page residency is not verified. There is no remote-network, sparse workspace, concurrent-agent, power-loss or cold-disk evidence here.

New binary SHA-256: `30de638e576f1764a8722c703084d8a7ac402a5d163c496a8b3fc2acd11d46c0`. Baseline: `6fe0b40248b9fe502b8939904948295f764b14d239ea480a6e559d609bdaa8b2`. Harness: `7249d9388e8bed2bd3c4977184c81167035ee7b4cb2f2b29b3656a426ee403d7`.

## Initial full commit, p50

| Layout | Files | Previous Loom ms | New Loom ms | Git workflow ms | Previous/new | Git/new |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 1,000 | 75.02 | 48.54 | 81.45 | 1.55× | 1.68× |
| sharded | 10,000 | 1241.24 | 855.87 | 1721.57 | 1.45× | 2.01× |
| nested | 10,000 | 1839.37 | 1000.49 | 1443.90 | 1.84× | 1.44× |
| sharded | 100,000 | 17218.76 | 8258.67 | 15624.83 | 2.08× | 1.89× |

Ratios above one favor the new Loom. Speed comes from independent publications, bounded duplicate elimination and reusing already encoded canonical buffers; individual file sync/readback requirements remain. No read-pack preparation, skipped bytes or Git pre-staging is hidden outside workflow timing.

## Eight-file change, p50

| Layout | Files | Previous Loom ms | New Loom ms | Git workflow ms | Previous/new | Git/new |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 1,000 | 11.08 | 10.45 | 24.78 | 1.06× | 2.37× |
| sharded | 10,000 | 77.01 | 76.88 | 35.51 | 1.00× | 0.46× |
| nested | 10,000 | 81.23 | 78.69 | 37.40 | 1.03× | 0.48× |
| sharded | 100,000 | 741.98 | 738.44 | 184.15 | 1.00× | 0.25× |

Commit-all still scans/hashes the worktree to discover changes. Fewer than 32 distinct pending objects use the serial path. The incremental rows remain an explicit limit: this change does not supply a change-sized workspace scan or prove the 100× target for commits.

## Initial CPU and raw child RSS

POSIX `wait4` supplies user/system CPU, raw child maximum RSS and kernel I/O counters. CPU includes all native worker threads. Parallelism can reduce elapsed time while consuming more aggregate CPU.

| Layout | Files | Previous CPU ms | New CPU ms | New/previous CPU | Previous wait4 RSS MiB | New wait4 RSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 1,000 | 73.89 | 45.73 | 0.62× | 20.10 | 20.10 |
| sharded | 10,000 | 1240.63 | 1717.58 | 1.38× | 34.25 | 34.25 |
| nested | 10,000 | 1816.44 | 2052.56 | 1.13× | 37.54 | 37.54 |
| sharded | 100,000 | 17001.83 | 16931.69 | 1.00× | 214.30 | 206.65 |

The RSS high-water may include the launcher's fork/pre-exec floor, visible in repeated equal values. These are raw process-level observations, **not isolated engine peak-RAM measurements**. No RSS reduction or cap is claimed. Window limits account owned canonical buffers; worktree inputs, encoding/current-object buffers, backend readback buffers, thread/runtime allocations and bookkeeping are additional.

## Kernel output counters and logical metadata size

Output counters use platform kernel units, not portable byte counts. Input counters and all Git resource samples are retained in JSON. Logical metadata lengths below are measured after both commits and verification; they do not measure disk allocation or storage-device traffic.

| Layout | Files | Previous oublock | New oublock | Previous metadata MiB | New metadata MiB | Git metadata MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 1,000 | 10380 | 10312 | 1.03 | 1.03 | 0.21 |
| sharded | 10,000 | 83328 | 83420 | 8.97 | 8.97 | 1.65 |
| nested | 10,000 | 89640 | 89728 | 9.27 | 9.29 | 1.73 |
| sharded | 100,000 | 880752 | 835848 | 88.82 | 88.82 | 16.02 |

## Implementation and correctness

`ObjectPublisher` buffers at most 128 changed canonical blobs and 8 MiB. One larger valid object runs serially under the existing 64 MiB payload cap. Each input is hash/envelope validated before I/O. Equal IDs share publication within a window; canonical byte collisions are rejected. Work is sorted by object ID for fanout locality. The provider hint defaults to one; filesystem suggests four. The core caps at eight and available CPUs, reusing discovery across windows.

Each object follows normal conditional create, sync and verified live readback. Scoped workers are all joined on error or panic. Blob windows finish before native roots, changesets and refs are published; dropping an operation never flushes pending objects. A failed operation may leave immutable orphans, and retry remains idempotent. Staged single-path publication retains its existing behavior. The shared scoped-worker helper also serves snapshot reads, whose protocol and bounds remain intact.

204 local tests pass with strict fmt/Clippy. Added cases exercise count/byte bounds, duplicate elimination, explicit finish/drop, invalid input before I/O, untrusted readback, serial defaults, excessive provider hints, worker caps and reused CPU discovery. Deterministic gates verify join-on-error/panic. A fault in the second publication window leaves the old branch/root mapping unchanged, drains active writes and permits a lossless retry. Existing cross-platform object and SQLite, snapshot/read-pack, framing, recovery and concurrency suites pass. CI additionally smoke-checks the POSIX publication harness on Linux.

The filesystem sync protocol itself is preserved, not power-loss certified by these tests. Provider declarations still require real durability support. Global multi-agent resource scheduling, remote storage, shared ref authority and the broader #26/#13 acceptance criteria remain open.

[Reproduction](../benchmarks/README.md#durable-publication-workflows), [publication contract](storage-backends.md#bounded-blob-publication). Raw evidence: [sharded](../benchmarks/results/publication-sharded-20261016.json), [nested](../benchmarks/results/publication-nested-20261016.json), [100k](../benchmarks/results/publication-100k-20261016.json).
