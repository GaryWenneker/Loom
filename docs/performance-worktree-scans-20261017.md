# Bounded full-content worktree scans — 2026-10-17

Legacy commit-all and status now read/hash through bounded windows instead of retaining every working-file payload. Every file is still read in full; this is not change-sized discovery or proof of universal 100× performance. All losses against Git and sequential controls remain visible.

Measured [Rust source d753cd08](https://github.com/GaryWenneker/Loom/commit/d753cd08f6c9fa37d629d58bd0b4d5b516006245), tree `aa50eaaa5ad34ab943c4a83678d46677159b0139`. Baseline is main #85, `b5262d10e9a1992929025b3c9b4b44e18a9fd435`. Follow-up evidence adds harness/CI/docs/results; production Rust is unchanged.

## Method and limits

Both binaries are release builds with Rust 1.99.1 on this Linux x86-64 AMD EPYC 9V74 host, nine reported CPUs. Kernel/runtime and filesystem behavior differ from the 16 October session; do not compare those timings directly. CPU governor, host scheduling and OS cache residency are uncontrolled; these are local measurements, not controlled hardware isolation.

Each complete-workflow engine/sample gets a fresh repository and identical unique binary bytes. New Loom uses automatic scan selection (up to four workers), baseline uses its existing serial full scan. Initial commit tracks every file; the next changes eight distributed files. Git times both add and commit, summing their startup/wall/CPU/I/O and taking maximum raw RSS. Native uses one commit-all command. Changed status is timed before the incremental commit; clean status follows verification. Output paths and every payload byte/frame are checked after commits, followed by full fsck. Old/new native root IDs must agree. Changed path sets and exact clean-status output are checked; the separate worker harness additionally checks complete sorted status bytes. Verification/setup time is excluded and can warm later access.

The 1k/10k default and nested cases have six rotated repeats (two complete engine-order cycles). The 100k and larger-payload controls have three, only one cycle. Percentiles are descriptive samples, not production tail SLOs. No controlled cold-disk, remote/network, concurrent-agent, sparse-workspace or power-loss evidence is claimed.

New binary SHA-256: `2c72dd6001a55bc40d6d556a7151f02524643191ea614590eb9e5b2d9f72fd30`; baseline `a00c859fbba06b1726624c4d7cf08e6a4f1157089b162ddb4d2eb9ffd9249c14`. Workflow harness SHA-256: `e73d044ad6582bca3f876212d5a9a1ed8c3a1c6bd4927f6fe9367205090244bd`. Git version and host metadata are in raw JSON.

## Initial full commit, p50

| Layout | Files | Previous Loom ms | New Loom ms | Git add+commit ms | Previous/new | Git/new |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 1,000 | 603.81 | 564.03 | 324.14 | 1.07× | 0.57× |
| sharded | 10,000 | 3,288.96 | 3,189.02 | 1,358.29 | 1.03× | 0.43× |
| nested | 10,000 | 3,555.06 | 3,617.89 | 1,736.34 | 0.98× | 0.48× |
| sharded | 100,000 | 30,782.21 | 29,400.47 | 14,296.04 | 1.05× | 0.49× |
| 64 KiB payload | 1,000 | 618.37 | 594.53 | 693.97 | 1.04× | 1.17× |

## Eight-file commit, p50

| Layout | Files | Previous Loom ms | New Loom ms | Git add+commit ms | Previous/new | Git/new |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 1,000 | 28.39 | 25.88 | 25.81 | 1.10× | 1.00× |
| sharded | 10,000 | 86.14 | 66.01 | 28.81 | 1.30× | 0.44× |
| nested | 10,000 | 93.83 | 70.62 | 31.89 | 1.33× | 0.45× |
| sharded | 100,000 | 678.71 | 436.46 | 157.40 | 1.56× | 0.36× |
| 64 KiB payload | 1,000 | 70.13 | 36.32 | 23.55 | 1.93× | 0.65× |

Ratios above one favor the new Loom. Small commits still scan every tracked directory/file and reconstruct the v1 state; staged commits remain the separate change-sized workflow. Initial object sync, verified readback, tree publication and ref CAS remain required.

## Status, p50

| Layout | Files | State | Previous ms | New ms | Git ms | Previous/new | Git/new |
| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| sharded | 1,000 | eight changed | 8.69 | 6.48 | 8.29 | 1.34× | 1.28× |
| sharded | 1,000 | clean | 8.67 | 6.99 | 8.03 | 1.24× | 1.15× |
| sharded | 10,000 | eight changed | 66.51 | 41.21 | 10.21 | 1.61× | 0.25× |
| sharded | 10,000 | clean | 64.86 | 41.17 | 12.52 | 1.58× | 0.30× |
| nested | 10,000 | eight changed | 69.59 | 45.26 | 14.85 | 1.54× | 0.33× |
| nested | 10,000 | clean | 68.80 | 44.58 | 15.62 | 1.54× | 0.35× |
| sharded | 100,000 | eight changed | 704.47 | 408.94 | 74.63 | 1.72× | 0.18× |
| sharded | 100,000 | clean | 709.51 | 423.02 | 65.86 | 1.68× | 0.16× |
| 64 KiB payload | 1,000 | eight changed | 52.32 | 13.29 | 5.63 | 3.94× | 0.42× |
| 64 KiB payload | 1,000 | clean | 46.68 | 12.95 | 6.38 | 3.60× | 0.49× |

## Requested scan-worker scaling

The separate status harness uses one committed filesystem fixture per size, then clean/eight-changed/all-changed phases. It checks exact sorted output, full working bytes, unchanged HEAD and fsck after each phase. Each setting gets seven samples; rotating seven settings gives one full order cycle. Requests 16/32/1,024 are all capped at eight/available CPUs, so their timing differences are noise/order effects rather than extra parallelism. Publication workers are not exercised by read-only status; this is an end-to-end scan control, not the object-worker microbenchmark or a Git comparison.

| Files | Requested workers | Clean ms | Eight changed ms | All changed ms | Serial/eight-changed | Eight-changed CPU ms |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 1 | 7.17 | 7.54 | 8.01 | 1.00× | 6.83 |
| 1,000 | 2 | 6.11 | 6.06 | 6.74 | 1.24× | 4.45 |
| 1,000 | 4 | 5.47 | 5.89 | 6.70 | 1.28× | 5.08 |
| 1,000 | 8 | 5.15 | 5.32 | 6.28 | 1.42× | 5.59 |
| 1,000 | 16 | 4.79 | 5.09 | 5.85 | 1.48× | 5.15 |
| 1,000 | 32 | 4.85 | 5.38 | 6.28 | 1.40× | 5.36 |
| 1,000 | 1024 | 5.02 | 5.14 | 6.02 | 1.47× | 5.66 |
| 10,000 | 1 | 55.57 | 60.52 | 70.07 | 1.00× | 60.11 |
| 10,000 | 2 | 47.59 | 49.79 | 56.37 | 1.22× | 40.81 |
| 10,000 | 4 | 40.93 | 42.18 | 49.51 | 1.43× | 37.84 |
| 10,000 | 8 | 35.34 | 40.10 | 45.68 | 1.51× | 49.60 |
| 10,000 | 16 | 37.73 | 38.53 | 44.76 | 1.57× | 47.88 |
| 10,000 | 32 | 38.43 | 37.82 | 43.97 | 1.60× | 45.75 |
| 10,000 | 1024 | 38.12 | 39.25 | 43.64 | 1.54× | 47.08 |
| 100,000 | 1 | 552.61 | 553.84 | 609.92 | 1.00× | 553.32 |
| 100,000 | 2 | 468.07 | 476.29 | 559.71 | 1.16× | 395.13 |
| 100,000 | 4 | 402.02 | 397.93 | 515.28 | 1.39× | 373.09 |
| 100,000 | 8 | 365.04 | 363.44 | 484.63 | 1.52× | 453.22 |
| 100,000 | 16 | 365.25 | 366.56 | 479.24 | 1.51× | 468.23 |
| 100,000 | 32 | 370.01 | 358.11 | 466.50 | 1.55× | 461.46 |
| 100,000 | 1024 | 363.22 | 367.97 | 443.86 | 1.51× | 456.42 |

Default selection was fixed before these measurements; no largest-worker-only result is used as the general default claim. Larger requests cannot guarantee lower wall time, and parallel CPU costs remain visible.

## Resources and storage

POSIX wait4 records per-process user/system CPU, raw maximum RSS and kernel I/O counters. CPU includes native threads; extra aggregate CPU can buy lower wall time. Raw RSS may include the Python launcher fork/pre-exec floor and is **not isolated engine RAM**. No RSS saving or total memory cap is inferred. Input/output counters are OS kernel units, not portable byte counts. Logical metadata lengths are measured after both commits/checks; they are not allocated disk space or device traffic. Complete distributions for all engines/operations are retained in JSON.

| Layout | Files | Previous initial CPU ms | New initial CPU ms | Previous raw RSS MiB | New raw RSS MiB | Previous metadata MiB | New metadata MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 1,000 | 528.03 | 504.51 | 20.19 | 20.19 | 0.99 | 0.99 |
| sharded | 10,000 | 3,048.29 | 3,117.07 | 34.33 | 34.33 | 8.93 | 8.93 |
| nested | 10,000 | 3,103.61 | 3,092.88 | 39.09 | 39.09 | 9.24 | 9.25 |
| sharded | 100,000 | 31,364.94 | 30,649.79 | 213.10 | 211.63 | 88.79 | 88.79 |
| 64 KiB payload | 1,000 | 410.79 | 390.21 | 265.25 | 265.11 | 63.74 | 63.75 |

## Correctness and remaining work

214 local tests pass with strict fmt/Clippy. New coverage checks serial/parallel native roots/change kinds across count windows, byte/large-file handling, binary/empty/duplicate contents, tracked ignores, post-stage edits, capped workers, length drift, failed read windows, rejected symlinks and oversized-input ref safety/retry. Existing second-publication-window fault injection retains the old ref/root mapping, joins active writes and retries losslessly. Linux CI smoke-checks both checked benchmark harnesses.

The read window owns at most 1,024 file records and 8 MiB discovered payload, with one larger valid file isolated. Reads fill canonical buffers directly; unchanged buffers are discarded inside workers. This payload bound excludes canonical headers/probes, runtime allocations, the separate publication window, history/path/deletion metadata, changeset/tree construction and the large-file exception. Other diff/materialization/recovery/merge paths retain their existing payload behavior.

External editors can race same-length reads; this is not an atomic filesystem snapshot. Unix leaf opens reject symlinks/FIFOs; parent traversal is not anchored, and the portable fallback does not establish race-free reparse confinement. Full scans still visit all paths and replay history. Change-sized discovery/indexing, global cross-process resource scheduling, remote storage/RPC, sparse workspaces, broad #26/#13 parity and 100× performance remain open.

[Scan contract](worktree-scans.md), [reproduction](../benchmarks/README.md#durable-publication-workflows). Raw evidence: [sharded](../benchmarks/results/worktree-sharded-20261017.json), [nested](../benchmarks/results/worktree-nested-20261017.json), [100k](../benchmarks/results/worktree-100k-20261017.json), [payload](../benchmarks/results/worktree-payload-20261017.json), [workers](../benchmarks/results/worktree-workers-20261017.json).
