# Atomic selected-set workflows — 2026-10-17

Explicit file/directory selection now prepares one atomic native staging set with bounded object publication. This removes repeated per-path lock/recovery/ignore/SQLite work and avoids republishing unchanged selected content. It does not add automatic change discovery or establish universal 100× performance.

Measured [source 45c3c692](https://github.com/GaryWenneker/Loom/commit/45c3c69243de93057e2398940f9194fd3d35537b), tree `3d84849969d89ce4fa23154353614f1078c52308`. Baseline is #86 main `a59f8be32d91b5188a15d48d24e90f1271b32283`; #87 changes documentation/evidence only, with identical production Rust. Final evidence commits do not change measured Rust.

## Method

Both release binaries use Rust 1.99.1 on the current Linux x86-64 Intel Xeon Platinum 8573C session (nine reported CPUs). Do not compare timings directly with earlier AMD-host reports. OS caches, CPU governor and host scheduling are uncontrolled. Benchmark engine commands are serialized; timing uses an uncontrolled shared host.

Each size has one existing filesystem-backed repository per engine. Initial full commits/fixture creation are outside timing. Both native seeds use the baseline writer and identical bytes; the initial derived branch index is absent. The first unchanged add is timed separately, capturing native cold index bootstrap. Repeated incremental samples use current indexes and identical history depth. This is not a claim that the first staging operation is change-sized. External-provider reopen/closure costs are not measured.

Each sample modifies the same eight files in eight directories (or fewer in tiny smoke fixtures). The files phase selects those exact paths. The directory phase selects their eight enclosing directories, including unchanged siblings. The legacy baseline has no recursive add; its equivalent candidate file list is pre-expanded outside timing, favoring the baseline. Each CLI keeps its normal progress output, so the legacy expanded list also emits more path lines. New Loom and Git receive the same file/directory selections. Complete workflows time **add plus commit**, including both CLI startups; add and commit samples are also retained separately. Engine order rotates across six repeats, two full cycles.

Every sample verifies the entire sorted path listing, selected binary payload bytes/framing and previous/new native root equality. After all samples, every final payload and full history/object closure fsck are checked for all engines. Verification/setup are excluded and can warm later access. Existing repositories accumulate the same number of commits in every engine. Six samples provide descriptive p50/p95/p99, not production tail SLOs.

New binary SHA-256 `2a471e23f853c171dd556d171df1cdfb5d2d2203d2acc8c8afcedaafa5e4e7d3`; baseline `2c72dd6001a55bc40d6d556a7151f02524643191ea614590eb9e5b2d9f72fd30`. Harness `9b3dcc3ba8020bb732d6180deab8898e806db644650b84bff93c92a1c20b92f3`; resource-helper hash and Git version are in raw JSON. Binaries were checked for recursive-add behavior before timing.

## Complete add+commit, p50

| Tracked files | Payload bytes | Selection | Candidate paths | Previous Loom ms | New Loom ms | Git ms | Previous/new | Git/new |
| ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 256 | files | 8 | 75.24 | 74.26 | 35.93 | 1.01× | 0.48× |
| 1,000 | 256 | directories | 32 | 111.92 | 82.85 | 42.18 | 1.35× | 0.51× |
| 10,000 | 256 | files | 8 | 147.25 | 112.96 | 109.40 | 1.30× | 0.97× |
| 10,000 | 256 | directories | 320 | 177.40 | 142.00 | 108.05 | 1.25× | 0.76× |
| 100,000 | 256 | files | 8 | 245.45 | 374.81 | 1,032.77 | 0.65× | 2.76× |
| 100,000 | 256 | directories | 3,128 | 1,214.59 | 361.94 | 704.16 | 3.36× | 1.95× |
| 1,000 | 65,536 | files | 8 | 101.39 | 116.24 | 45.39 | 0.87× | 0.39× |
| 1,000 | 65,536 | directories | 32 | 277.88 | 251.03 | 89.80 | 1.11× | 0.36× |

Ratios above one favor the new Loom. All unfavorable Git/baseline controls are retained. These are explicitly selected native workflows; legacy commit-all and informational full-worktree status retain the scan behavior measured in the separate scan report.

## Add and commit split, p50

| Files | Payload bytes | Selection | Previous add ms | New add ms | Previous commit ms | New commit ms |
| ---: | ---: | --- | ---: | ---: | ---: | ---: |
| 1,000 | 256 | files | 42.31 | 28.41 | 36.33 | 40.70 |
| 1,000 | 256 | directories | 59.78 | 33.58 | 55.47 | 39.76 |
| 10,000 | 256 | files | 62.83 | 49.93 | 80.59 | 62.11 |
| 10,000 | 256 | directories | 106.25 | 62.25 | 66.81 | 87.41 |
| 100,000 | 256 | files | 124.08 | 125.41 | 95.79 | 194.10 |
| 100,000 | 256 | directories | 1,100.51 | 249.09 | 114.08 | 75.10 |
| 1,000 | 65,536 | files | 49.43 | 43.73 | 52.88 | 52.30 |
| 1,000 | 65,536 | directories | 93.32 | 69.82 | 57.13 | 82.12 |

Split medians do not necessarily sum to the median complete workflow. Unchanged selected content no longer incurs live republishing; new/changed data retains conditional creation, sync and verified readback.

## First unchanged add and derived-index bootstrap

Each row below is **one cold-index observation**, outside repeated incremental samples. It includes startup and the native full-history/path-index rebuild when absent; Git already has its native index from repository creation. No distribution or bootstrap speed guarantee is inferred.

| Files | Payload bytes | Previous first add ms | New first add ms | Git first add ms |
| ---: | ---: | ---: | ---: | ---: |
| 1,000 | 256 | 38.87 | 21.64 | 5.45 |
| 10,000 | 256 | 48.87 | 105.27 | 21.10 |
| 100,000 | 256 | 602.91 | 692.43 | 344.71 |
| 1,000 | 65,536 | 36.50 | 17.65 | 10.73 |

## Resources

POSIX wait4 captures per-process CPU, raw maximum RSS and kernel I/O counters. Complete-workflow wall/CPU/I/O sum both commands; RSS uses their maximum. Raw RSS may include the Python launcher fork/pre-exec floor and is **not isolated engine RAM**. No total RSS cap or RAM-saving claim is made. Kernel I/O counters are platform units, not portable byte counts. Logical metadata size after final verification is retained, not allocated storage or device traffic. No controlled cold-storage, network, sparse workspace, concurrent-agent or crash/power-loss evidence is provided.

| Files | Payload bytes | Selection | Previous CPU ms | New CPU ms | Git CPU ms | Previous raw RSS MiB | New raw RSS MiB |
| ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 256 | files | 28.91 | 26.50 | 33.30 | 19.75 | 19.75 |
| 1,000 | 256 | directories | 50.11 | 36.59 | 38.77 | 19.75 | 19.75 |
| 10,000 | 256 | files | 59.40 | 47.93 | 130.47 | 20.56 | 20.56 |
| 10,000 | 256 | directories | 96.18 | 71.12 | 133.28 | 20.56 | 20.56 |
| 100,000 | 256 | files | 93.48 | 94.55 | 1,973.24 | 39.06 | 39.06 |
| 100,000 | 256 | directories | 1,034.13 | 301.49 | 1,661.84 | 39.06 | 39.06 |
| 1,000 | 65,536 | files | 48.24 | 50.16 | 39.86 | 19.62 | 19.62 |
| 1,000 | 65,536 | directories | 170.06 | 145.40 | 85.34 | 19.62 | 19.62 |

## Correctness and remaining work

226 local tests pass with strict fmt/Clippy. Added coverage includes 5,000 explicit input paths, duplicate/overlapping/root selections, exact prefix boundaries, tracked and prepared ignore behavior, deleted additions/subtrees, both file-directory replacement directions, post-stage immutable bytes, unrelated oversized files, atomic CLI unstage, invalid arguments, SQL rollback/staging-mode activation, provider failures/readback corruption, moved refs and lossless retry. Linux CI smoke-checks complete selected-set workflows.

One worktree lock covers preparation and pinning. All selected blobs complete their bounded verified publication before one SQLite stage transaction checks the captured branch/ref, enables explicit staging and updates selected rows. Unselected staging is retained. Reader/provider/SQL/ref errors retain prior staged data; immutable orphans and rebuildable bootstrap indexes can remain. Existing commit publishes native tree/ref/path-index changes in its normal atomic transaction.

Candidate path metadata and branch-index bootstrap are repository/selection-sized; the read/publish payload windows are not total RSS caps. Known v1 external-editor and unanchored parent-traversal limitations remain. Applications must supply the complete intended selection. Automatic journals/change discovery, native remote server, broad #26/#13 parity and the 100× Git performance goal remain open.

[Selection contract](pathset-staging.md), [reproduction](../benchmarks/README.md#atomic-selected-set-workflows). Raw evidence: [small](../benchmarks/results/pathsets-small-20261017.json), [100k](../benchmarks/results/pathsets-100k-20261017.json), [larger payload](../benchmarks/results/pathsets-payload-20261017.json).
