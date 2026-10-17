# Nested subtree construction — 2026-10-17

Native snapshot construction now follows common prefixes to independent directory edits, instead of limiting workers to at least 32 root entries. Cheap file edits stay serial; descendants of a fork stay serial; the storage provider write hint caps the whole pool. This activates useful work for selections below a shared parent and prevents subtree × replica worker multiplication. Canonical IDs, CAS and staged snapshot semantics are unchanged.

## Source and scope

Local measurements pin [source 71d1cd76](https://github.com/GaryWenneker/Loom/commit/71d1cd76455cab2a5cd390db5767354acf2ad806), tree `544943ee695386f67587bb67c59fdd11bad29625`. Baseline is main `5faf141bad40136c4e87858f1de17af5d8a74642`, tree `abad9544d6f430ca5fa3ba35f4a40effc5521f74`. Candidate release SHA-256 `863470a6815e277bd171a4790bf1d0780df787e29ac812b93d1ccedd4534675b`; baseline `117e876a69759ad566c84defc16ee9aee1f524d7c276992627c6a0af38b49619`. Both use Rust 1.99.1 on the current Intel Xeon Platinum 8573C / Linux 6.18.55 host (nine reported CPUs).

After these isolated measurements, [integrated source e95cbcc4](https://github.com/GaryWenneker/Loom/commit/e95cbcc4a09d9da1771ae9d75a8b61045e585d19) incorporates the independent adaptive text-merge change from #93 and replaces a timer-based replica overlap test with explicit synchronization. Those changes do not alter the measured add/commit/tree implementation or harness. The local timing data remains attributed to the original measured binary; the final exact-head CI workflow rebuilds and checks the combined candidate at 1k/10k/100k. Do not substitute one binary hash for the other.

## Method

Each run compares complete add-plus-commit workflows, including both startups, against previous Loom and Git on identical files and histories. Sixty-four files change in sixty-four directories below the shared files/ prefix. Files and enclosing directory scopes are measured separately. The baseline receives an equivalent expanded file list computed outside timing, favoring it; normal CLI progress output remains enabled. The baseline supports recursive selection but the expanded control is retained.

The initial repositories are written by the baseline and initial derived path indexes are absent. One first unchanged add/bootstrap is recorded separately; repeated timings use current indexes. Initial fixture creation, verification and cold-index bootstrap are excluded from incremental distributions. Every sample checks the full path list, all changed bytes/framing and native root equality. Every final byte and complete history/closure fsck are checked after the samples.

Six repeats rotate baseline/new/Git through two full engine-order cycles. Worker requests 1 and 8 are separate runs, not interleaved worker trials. They apply to candidate read/hash and subtree preparation; buffered object publication retains its own provider limit. The previous binary keeps its default worker policy. Request 1 therefore does not make the whole engine single-threaded, and request 8 does not bypass the filesystem provider maximum of four subtree writers or the CPU cap.

No local builds or tests run concurrently with the reported measurements. This is still a shared host with uncontrolled OS caches, device scheduling and CPU scheduling. For example, the unchanged 1k-file baseline file-selection p50 changes from 252.25 to 557.41 ms between worker runs. Do not infer a causal worker-1/worker-8 speedup from separate run medians. Interpret paired engine controls and retain their variation.

## Complete add plus commit, p50

| Request | Files | Payload bytes | Selection | Candidate paths | Previous Loom ms | New Loom ms | Git ms | Previous/new | Git/new |
| ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 1,000 | 256 | files | 64 | 252.25 | 267.82 | 68.93 | 0.94× | 0.26× |
| 1 | 1,000 | 256 | directories | 256 | 250.81 | 226.23 | 58.34 | 1.11× | 0.26× |
| 1 | 10,000 | 256 | files | 64 | 250.18 | 299.74 | 128.06 | 0.83× | 0.43× |
| 1 | 10,000 | 256 | directories | 2,512 | 496.13 | 312.85 | 126.95 | 1.59× | 0.41× |
| 8 | 1,000 | 256 | files | 64 | 557.41 | 502.34 | 169.76 | 1.11× | 0.34× |
| 8 | 1,000 | 256 | directories | 256 | 414.65 | 237.83 | 136.92 | 1.74× | 0.58× |
| 8 | 10,000 | 256 | files | 64 | 272.61 | 179.98 | 167.68 | 1.51× | 0.93× |
| 8 | 10,000 | 256 | directories | 2,512 | 423.51 | 218.87 | 184.63 | 1.93× | 0.84× |
| 8 | 1,000 | 65,536 | files | 64 | 346.26 | 320.18 | 267.55 | 1.08× | 0.84× |
| 8 | 1,000 | 65,536 | directories | 256 | 483.31 | 356.30 | 267.10 | 1.36× | 0.75× |

Ratios above one favor new Loom. The 10k / 64-directory request-8 case improves from 423.51 to 218.87 ms (1.93× versus previous Loom); Git is 184.63 ms and remains faster. The request-1 exact-file controls regress at both sizes. Every tested Git default case is faster than new Loom. All controls remain in the table; neither 100× Git performance nor a universal improvement is established.

## Add and commit split, p50

| Request | Files | Payload bytes | Selection | Previous add ms | New add ms | Previous commit ms | New commit ms |
| ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: |
| 1 | 1,000 | 256 | files | 78.04 | 99.72 | 177.32 | 153.93 |
| 1 | 1,000 | 256 | directories | 90.14 | 76.28 | 165.77 | 153.28 |
| 1 | 10,000 | 256 | files | 78.10 | 92.50 | 172.31 | 207.01 |
| 1 | 10,000 | 256 | directories | 312.06 | 150.52 | 212.54 | 158.36 |
| 8 | 1,000 | 256 | files | 235.73 | 192.46 | 289.13 | 281.89 |
| 8 | 1,000 | 256 | directories | 141.29 | 113.85 | 261.79 | 131.13 |
| 8 | 10,000 | 256 | files | 87.88 | 88.40 | 184.90 | 90.59 |
| 8 | 10,000 | 256 | directories | 236.94 | 125.53 | 187.19 | 94.88 |
| 8 | 1,000 | 65,536 | files | 125.64 | 143.19 | 203.82 | 127.68 |
| 8 | 1,000 | 65,536 | directories | 188.51 | 170.38 | 241.42 | 152.75 |

Split medians need not sum to the complete-workflow median. Complete-workflow comparisons include changes in selection, read/hash worker request and subtree construction; these timings do not isolate one internal function.

## CPU and memory observations

| Request | Files | Payload bytes | Selection | Previous CPU ms | New CPU ms | Git CPU ms | Previous raw RSS MiB | New raw RSS MiB |
| ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 1,000 | 256 | files | 105.75 | 98.26 | 63.61 | 19.85 | 19.85 |
| 1 | 1,000 | 256 | directories | 106.68 | 99.70 | 54.09 | 19.85 | 19.85 |
| 1 | 10,000 | 256 | files | 107.23 | 120.39 | 154.36 | 20.84 | 20.84 |
| 1 | 10,000 | 256 | directories | 328.47 | 174.57 | 149.24 | 20.84 | 20.84 |
| 8 | 1,000 | 256 | files | 405.37 | 364.15 | 162.20 | 19.86 | 19.86 |
| 8 | 1,000 | 256 | directories | 264.46 | 227.68 | 132.93 | 19.86 | 19.86 |
| 8 | 10,000 | 256 | files | 165.29 | 186.14 | 195.40 | 20.66 | 20.66 |
| 8 | 10,000 | 256 | directories | 311.31 | 219.77 | 205.22 | 20.66 | 20.66 |
| 8 | 1,000 | 65,536 | files | 286.44 | 338.49 | 246.94 | 22.14 | 22.14 |
| 8 | 1,000 | 65,536 | directories | 392.37 | 381.33 | 261.52 | 22.27 | 22.27 |

Raw JSON retains p50/p95/p99 and individual samples for both command timings, process CPU, raw wait4 maximum RSS and kernel I/O counters, plus first-add/bootstrap observations, canonical roots and final logical metadata bytes. RSS may include the Python fork/pre-exec launcher floor and is not isolated engine memory. Kernel block counters are not portable byte counts; logical metadata bytes are not allocated device blocks. Payload and worker limits are not total RSS caps.

The fixtures use normal engine defaults. This comparison does not establish identical power-loss guarantees between Loom and Git. Controlled cold storage, network traffic, remote/cloud providers, sparse workspaces, concurrent agents, sustained load and crash/power-loss recovery are unmeasured. Six samples provide descriptive quantiles, not production tail SLOs.

## Correctness and remaining work

The isolated source passed 239 local workspace/all-target tests, formatting and strict Clippy. The integrated source passes 240 local tests, including the independent text-merge policy and deterministic replica overlap test. Nested-tree tests require real concurrent writes below a shared prefix, enforce one/two-worker provider limits, rule out recursive pool multiplication, and compare every path and canonical root. A native-tree write failure retains the branch and prepared snapshot, drains workers and succeeds on retry.

The implementation remains an experimental local alpha. #26/#13, native v2 migration, secure remote authority, sparse isolated agent workspaces, broad fault/soak coverage and the 100× Git target remain open. No overall completion percentage is inferred from passing these checks.

[Subtree contract](subtree-workers.md). Raw results: [request 1](../benchmarks/results/nested-subtrees-workers-1-20261017.json), [request 8](../benchmarks/results/nested-subtrees-workers-8-20261017.json), [larger payload](../benchmarks/results/nested-subtrees-payload-20261017.json). [Source/compiler/binary/data provenance](../benchmarks/results/nested-subtrees-provenance-20261017.json).
