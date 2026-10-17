# Nested subtree construction — corrected comparison, 2026-10-17

The same-input 10k-file / 64-directory workflow records 321.27 ms for previous Loom and 247.22 ms for the candidate: **1.30× previous Loom**. Git records 119.65 ms, about **2.07× faster than the candidate**. The serial controls are neutral or slower, and the 100× Git target is not achieved. All measured cases appear below.

The earlier **1.93× conclusion is withdrawn** as a like-for-like speedup. That harness expanded the baseline's directory scopes into file arguments despite its native directory support and did not pair worker requests. The [historical report](performance-nested-subtrees-expanded-control-20261017.md), all three original raw files and their original provenance remain available, explicitly labeled as adapted controls. Replacement measurements below use identical native inputs.

## Implementation

Native tree construction follows common prefixes to independent changed directories, instead of forking only at the repository root with at least 32 edits. Cheap file-entry edits stay serial; directory tasks are grouped separately. Once a node forks, its descendants run serially, preventing recursive worker multiplication. The same bounded provider write hint caps both subtree construction and buffered object publication. Replicated providers retain their outer one-write hint. Canonical ordering, IDs, staged snapshots and ref CAS remain unchanged.

These are per-call bounds, not a global concurrency or RSS budget. Scanning, object publication and subtree construction are joined phases. Failed subtree builds drain workers and preserve the prepared snapshot/ref for retry; unreachable immutable objects can remain after failure.

## Source and method

Local measurements pin [candidate 91bb55ea](https://github.com/GaryWenneker/Loom/commit/91bb55ea7691fa0eaf22af783597a90f79f316d2), tree `ef8ddc0a802602433bff5d392498ed283ab132a6`. Baseline is main `6df6411c4262cf8a5e81d7d399b56d462b6b1d63`, tree `2389a12d26e4342d6691f3c81dd30f13c01bcd60`. Both include #93's adaptive text merges. Candidate release SHA-256 is `d415de39639466ddaa3736bf29f07f20f2c79a29ee87e56b3d69b5cbd01577ef`; baseline is `79c56ea8bf561d6d62171478488086aca7177a6666101f26f9b6f68eda243857`. Both were rebuilt separately with Rust 1.99.1 on Intel Xeon Platinum 8573C / Linux 6.18.55, nine reported CPUs, eight-core cgroup quota. Cargo core/CLI release products were cleaned between builds to avoid shared-target reuse.

After these measurements, [integrated source 51b97632](https://github.com/GaryWenneker/Loom/commit/51b976324cc28e8f3958c5f7b8314486dcc3c111) incorporates #95's atomic SQLite object batches from main `3a607fd8a5ded59dbf919a321a50339851b0993a`. This adds a batch-provider probe before the filesystem fallback. The local timing numbers remain attributed to source 91bb, not the integrated binary. Final exact-head CI rebuilds and measures the combined source against main 3a607 at 1k/10k/100k, with live replica and SQLite workflow evidence checked separately. Do not substitute source or binary identities.

Each measurement includes complete add plus commit, including both CLI startups and normal progress output. Sixty-four files change across sixty-four directories below a shared `files/` prefix. Exact files and their enclosing directory scopes are separate phases. Both native executables receive identical paths and identical explicit `--scan-workers` values on **both add and commit**. `--baseline-directories` disables the legacy expanded-file adapter. An executable-wrapper regression compares the actual timed argument lists; it passes with these distinct rebuilt binaries and runs again in CI against both built versions.

Initial setup/full commits are outside timing. Repositories begin with identical history written by the baseline. The first unchanged add/index bootstrap is recorded separately; repeated samples use current indexes. Each of six repeats rotates baseline/new/Git order. Every sample checks the full listing, changed bytes/framing and equal native roots. Final verification reads every byte and runs full history/closure fsck. Root sequences also match across the worker-0/worker-1 controls.

Worker request 0 selects bounded automatic scheduling; request 1 serializes selected read/hash and tree work. Buffered publication retains its own provider policy, so request 1 is not a whole-engine single-thread claim. These worker runs are separate, not interleaved worker experiments. No local builds/tests overlapped the measured runs, but caches, device and CPU scheduling on this shared host remain uncontrolled. Do not infer causal worker-policy improvements by dividing separate-run medians.

## Complete add plus commit, p50

| Both requests | Files | Payload bytes | Selection | Previous Loom ms | New Loom ms | Git ms | Previous/new | Git/new |
| ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 0 | 1,000 | 256 | files | 373.88 | 251.62 | 71.00 | 1.49× | 0.28× |
| 0 | 1,000 | 256 | directories | 952.41 | 399.22 | 204.61 | 2.39× | 0.51× |
| 0 | 10,000 | 256 | files | 298.98 | 209.08 | 118.25 | 1.43× | 0.57× |
| 0 | 10,000 | 256 | directories | 321.27 | 247.22 | 119.65 | 1.30× | 0.48× |
| 1 | 1,000 | 256 | files | 345.24 | 429.59 | 111.89 | 0.80× | 0.26× |
| 1 | 1,000 | 256 | directories | 320.29 | 329.49 | 113.50 | 0.97× | 0.34× |
| 1 | 10,000 | 256 | files | 246.99 | 253.54 | 142.45 | 0.97× | 0.56× |
| 1 | 10,000 | 256 | directories | 332.42 | 334.32 | 171.52 | 0.99× | 0.51× |
| 0 | 1,000 | 65,536 | files | 520.28 | 277.15 | 305.00 | 1.88× | 1.10× |
| 0 | 1,000 | 65,536 | directories | 396.11 | 279.19 | 198.20 | 1.42× | 0.71× |

Ratios above one favor new Loom. All directory cases remain slower than Git. The 64-KiB exact-file case records 1.10× Git in this run; this does not establish a general win or approach 100×. The 1k automatic directory case records 2.39× previous Loom but has pronounced variation (baseline p50 952 ms versus 320 ms in the separate serial run). Six repeats on a shared host do not establish a stable production speedup. The exact-file serial control regresses at both sizes; directory serial controls are nearly unchanged/slower.

## Add and commit split, p50

| Both requests | Files | Payload bytes | Selection | Previous add ms | New add ms | Previous commit ms | New commit ms |
| ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: |
| 0 | 1,000 | 256 | files | 125.76 | 122.94 | 196.12 | 114.03 |
| 0 | 1,000 | 256 | directories | 208.08 | 158.33 | 616.87 | 234.35 |
| 0 | 10,000 | 256 | files | 86.61 | 90.32 | 207.99 | 120.67 |
| 0 | 10,000 | 256 | directories | 129.03 | 141.60 | 202.82 | 98.11 |
| 1 | 1,000 | 256 | files | 120.89 | 132.36 | 226.07 | 301.82 |
| 1 | 1,000 | 256 | directories | 116.77 | 107.40 | 199.07 | 200.75 |
| 1 | 10,000 | 256 | files | 72.08 | 74.20 | 174.51 | 183.20 |
| 1 | 10,000 | 256 | directories | 136.78 | 163.47 | 183.12 | 196.30 |
| 0 | 1,000 | 65,536 | files | 226.90 | 145.66 | 275.39 | 143.82 |
| 0 | 1,000 | 65,536 | directories | 160.78 | 150.88 | 235.33 | 135.13 |

Split medians need not sum to complete-workflow medians. Wall timings alone do not isolate subtree construction from other workflow costs.

## CPU and raw memory observations, p50

| Both requests | Files | Payload bytes | Selection | Previous CPU ms | New CPU ms | Git CPU ms | Previous raw RSS MiB | New raw RSS MiB | Git raw RSS MiB |
| ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 1,000 | 256 | files | 148.37 | 166.50 | 67.79 | 19.90 | 19.90 | 19.90 |
| 0 | 1,000 | 256 | directories | 482.35 | 228.95 | 150.09 | 19.90 | 19.90 | 19.90 |
| 0 | 10,000 | 256 | files | 120.91 | 132.84 | 137.71 | 20.71 | 20.71 | 20.71 |
| 0 | 10,000 | 256 | directories | 173.31 | 177.62 | 140.39 | 20.71 | 20.71 | 20.71 |
| 1 | 1,000 | 256 | files | 173.84 | 215.97 | 107.66 | 19.90 | 19.90 | 19.90 |
| 1 | 1,000 | 256 | directories | 197.34 | 184.14 | 108.03 | 19.90 | 19.90 | 19.90 |
| 1 | 10,000 | 256 | files | 150.81 | 151.69 | 158.72 | 20.71 | 20.71 | 20.71 |
| 1 | 10,000 | 256 | directories | 206.29 | 232.76 | 191.60 | 20.71 | 20.71 | 20.71 |
| 0 | 1,000 | 65,536 | files | 432.47 | 340.39 | 273.14 | 22.02 | 22.02 | 22.02 |
| 0 | 1,000 | 65,536 | directories | 312.90 | 310.06 | 192.91 | 22.02 | 22.02 | 22.02 |

Raw JSON retains individual samples, p50/p95/p99, wall/CPU, raw wait4 RSS and kernel I/O counters, bootstrap observations, root sequences and final logical metadata bytes. RSS includes a possible Python fork/pre-exec launcher floor and cannot substantiate isolated engine memory savings. Kernel I/O counters are platform units, not portable byte counts. Logical metadata bytes are not allocated device blocks. Six-sample tail quantiles are descriptive, not production SLOs.

Normal engine defaults do not establish identical power-loss contracts between Loom and Git. Controlled cold-disk, network/cloud providers, sparse workspaces, concurrent agents, sustained load and crash/power-loss recovery remain unmeasured.

## Correctness and remaining work

The measured source passed 240 local workspace/all-target tests, formatting and strict Clippy. The final combined source requires a clean isolated local workspace/all-target test build, formatting, strict Clippy, and Linux/macOS/Windows, Rust 1.85 minimum and security CI on the final PR head. PR #94 records the completed exact-head and resulting-main checks; timing evidence alone is not the merge gate. Provider probes require real overlap under a shared prefix, enforce one/two-worker limits, verify canonical roots/full paths and reject recursive pool multiplication. Fault tests preserve prepared snapshots and refs, drain workers and verify successful retries. The former replica overlap test now synchronizes explicitly instead of assuming a 25-ms sleep schedules every worker.

Loom remains an experimental local alpha. #26/#13, native v2 migration, secure remote authority, isolated sparse agent workspaces, broad fault/soak coverage and the 100× Git performance target remain open. Passing current checks does not establish 100% product completion.

[Subtree contract](subtree-workers.md). Corrected raw results: [both automatic](../benchmarks/results/nested-subtrees-fair-workers-0-20261017.json), [both serial](../benchmarks/results/nested-subtrees-fair-workers-1-20261017.json), [larger payload](../benchmarks/results/nested-subtrees-fair-payload-20261017.json). [Source/compiler/binary/data provenance](../benchmarks/results/nested-subtrees-fair-provenance-20261017.json).
