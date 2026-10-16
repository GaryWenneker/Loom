# Multi-directory read improvements — 2026-10-16

Native grouped reads improve these workloads by about 2–3× over previous Loom.
The existing line interface also improves. The broad 100× Git target remains
**open**; none of these multi-directory controls meets it.

Measured source: [446fc878](https://github.com/GaryWenneker/Loom/commit/446fc878f5ae34bdad4294ac0c9495e806732a31),
tree `55ac7912ac1fd4c3585c7d2d0517a87c341557d9`.
Previous Loom is main `8b1e961f93907562b0a076fc89bfdf7d54845349`.
The evidence-only follow-up does not change measured Rust or harness source.

## Method

Seven repeats per scenario; 1,000 identical deterministic logical path requests
with 10% misses. The native operation groups those paths in one `cat_many`
request; old/new line modes send individual cat requests. Git uses its own
persistent `cat-file --batch`, packed objects and a changed-path commit graph.
All modes start a fresh process and include startup; OS caches are warm. Engine
order rotates and every requested byte/missing slot/frame is checked.
The previous binary initializes the shared fixture, proving existing objects
need no repacking or format migration. There are N generated tracked fixture
files plus one noise file and 11 commits in both repositories.

`sharded` uses 256 leaf directories (`files/bucket/name`). `nested` uses 1,024
leaf directories beneath another 16-way level (`files/first/second/name`).
Files are small binary values, not a representative source-code or large-file
corpus. These numbers do not establish network, cold-storage, multi-agent or
arbitrary repository performance. Seven samples provide only rough tail estimates;
raw p95/p99 remain descriptive, not certified tail latency.

Linux x86-64 container, AMD EPYC 9V74, 9 reported CPUs (8-CPU container quota),
overlay filesystem; Rust 1.99.1,
release profile, Git 2.51.1. CPU/RSS/I/O-byte costs were not measured here. Both
binaries use the same toolchain and fixture. Binary and harness hashes are in
all raw reports; new binary SHA-256: `306e683806e89e7880b721b27e5fc0cfee6ebe1f2bba29d7e4a3b6accaeab62b`.

## End-to-end p50 milliseconds

| Layout | Fixture files | Leaf dirs | Previous Loom lines | New Loom lines | New Loom grouped | Git batch |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 1,000 | 256 | 21.45 | 11.94 | 7.25 | 40.18 |
| sharded | 10,000 | 256 | 24.06 | 16.86 | 9.06 | 48.50 |
| sharded | 100,000 | 256 | 39.00 | 27.56 | 16.28 | 84.24 |
| nested | 10,000 | 1,024 | 27.31 | 20.68 | 13.64 | 26.76 |

## Operation-specific speed ratios

| Layout | Fixture files | New lines vs previous | Grouped vs previous lines | Grouped vs Git batch |
| --- | ---: | ---: | ---: | ---: |
| sharded | 1,000 | 1.80× | 2.96× | 5.54× |
| sharded | 10,000 | 1.43× | 2.66× | 5.35× |
| sharded | 100,000 | 1.41× | 2.39× | 5.17× |
| nested | 10,000 | 1.32× | 2.00× | 1.96× |

Grouping changes framing and error semantics: a group fails atomically while
individual line requests can succeed/fail separately. This is a native API
capability, not a claim that unchanged clients get the grouped-mode speed.

## Single-command controls

Git/new-Loom ratios below 1 mean Loom is slower. These results are retained:
status remains a full content scan, and individual CLI reads are not uniformly
faster. Grouped immutable reading does not resolve those costs.

| Layout | Fixture files | cat Git/Loom | files Git/Loom | status Git/Loom |
| --- | ---: | ---: | ---: | ---: |
| sharded | 1,000 | 1.04× | 1.02× | 0.61× |
| sharded | 10,000 | 1.06× | 1.04× | 0.19× |
| sharded | 100,000 | 1.07× | 0.98× | 0.11× |
| nested | 10,000 | 0.79× | 0.77× | 0.21× |

## Accounted object-cache footprint

For the same 10k-file/256-directory fixture and 1,000 paths, accounted retained
object-cache bytes changed from 1,355,270 to 739,118 (45.5% lower).
This measures verified retained content plus estimated bookkeeping, **not RSS**;
pinned directory handles and temporary buffers are separate. Returned bytes were
checked and both views loaded the same logical data. See the
[cache-accounting sample](../benchmarks/results/multidir-cache-accounting-20261016.json).

Raw timing samples: [sharded](../benchmarks/results/multidir-sharded-20261016.json)
and [nested](../benchmarks/results/multidir-nested-20261016.json).
[Reproduction](../benchmarks/README.md#multi-directory-beforeafter-comparison)
and [API/framing/resource contract](snapshot-query-protocol.md#native-grouped-file-reads).

Further gains require reducing remaining per-object storage work and query
startup costs. The separate status/write/storage-format and production gates
remain open. These local observations are not a general Git/Scion speed guarantee.
