# Measured Git comparison — 2026-10-16

The broad 100× performance goal remains **open**. A wide-directory batch meets
it; the sharded control, status and individual CLI operations do not. These are
local observations for selected workloads, not general speed guarantees.

## Exact candidate and conditions

Measured source tree: `fa6eb9458e2aec2c7f6c5b84da67fe774058a79c`, published as
[fc287a5](https://github.com/GaryWenneker/Loom/commit/fc287a5eb776a93b40a58f91d68136bc1b7b7af2).
Later evidence/documentation changes do not alter the measured Rust or harness
source. Release binary SHA-256: `dcc5d7c1b30c2ac7194ed5c8c63fccab080bb4ebe51bb427290aada4aaccf5f5`.
Harness SHA-256: `49962929ebaa1b2b5d82c6abe90253acd2229677bda6bef89fdacc8bb264f77c`.

Linux 6.18.44 x86-64 container, Intel Xeon Platinum 8573C, 9 available CPUs,
8 GiB cgroup memory limit, local overlay filesystem. Rust 1.99.1; Git 2.51.1.
Both repositories contain identical tracked small binary files and 11 commits.
Git uses packed objects plus a reachable changed-path commit graph. Queries pin
immutable revisions. Each batch has 1,000 deterministic path requests with 10%
misses; every requested output byte and response frame is checked. Git uses one
`git cat-file --batch` process, Loom one `query --batch` process with its default
64 MiB accounted cache. The sharded control spreads files over 256 directories.

Three repetitions per scenario; engine order alternates, startup is included,
and OS caches are warm. Shared host scheduling is not isolated. p95/p99 are
included as descriptive interpolations in the raw samples; three repetitions
cannot establish reliable tail latency. CPU time, peak RSS, cold disk, network,
fault tolerance and multi-client throughput were not measured here.

## Batch wall time

| Layout | Tracked files | Loom p50 ms | Git p50 ms | Git / Loom | 100× target met |
| --- | ---: | ---: | ---: | ---: | --- |
| wide | 1,000 | 18.37 | 202.38 | 11.02× | no |
| wide | 10,000 | 30.51 | 1925.18 | 63.10× | no |
| wide | 100,000 | 73.10 | 21907.00 | 299.69× | yes |
| sharded | 10,000 | 32.33 | 78.96 | 2.44× | no |

## Individual CLI operations

Ratios below 1 mean Loom is slower. `cat` reads one file, `files` lists an exact
file prefix, and `status` checks the clean worktree. These measurements retain
unfavorable results. Native status still scans contents; at 100,000 files it is
about 12× slower than Git in this run. Read-session acceleration does not fix it.

| Layout | Tracked files | cat Git / Loom | files Git / Loom | status Git / Loom |
| --- | ---: | ---: | ---: | ---: |
| wide | 1,000 | 0.93× | 1.12× | 0.59× |
| wide | 10,000 | 0.84× | 0.68× | 0.23× |
| wide | 100,000 | 3.55× | 0.61× | 0.08× |
| sharded | 10,000 | 0.69× | 0.89× | 0.20× |

## Storage footprint

Logical bytes in the complete metadata directories after the scenario, including
indexes/journals and immutable objects. These are not filesystem allocated bytes
or peak temporary usage, and are not object-only compression comparisons.
Loom's loose files also incur per-file allocation/inode overhead. Read caching
and read-only SQLite sessions do not solve write amplification or compaction.

| Layout | Tracked files | .loom logical bytes | .git logical bytes |
| --- | ---: | ---: | ---: |
| wide | 1,000 | 405,571 | 220,399 |
| wide | 10,000 | 2,978,603 | 1,884,586 |
| wide | 100,000 | 28,823,611 | 18,704,100 |
| sharded | 10,000 | 3,076,955 | 1,943,951 |

Raw samples: [wide](../benchmarks/results/query-final-wide-20261016.json) and
[sharded](../benchmarks/results/query-final-sharded-20261016.json).
[Reproduction commands](../benchmarks/README.md) describe the fixture and controls.
Next priorities remain trustworthy incremental status, durable batched writes,
portable large-file storage, writable workspace coordination and independent
fault/performance evidence. No overall Git or Scion parity is claimed.
