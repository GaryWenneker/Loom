# Optional read-pack performance — 2026-10-16

An explicit immutable read pack reduces object-open overhead for grouped reads. The full-index startup cost limits its benefit at 100k files. The general 100× Git target remains **open**; none of these multi-directory controls reaches it.

Measured [code fa5be55a](https://github.com/GaryWenneker/Loom/commit/fa5be55a9626a452a7300590561dbccd0f0efe67), tree `a8f40bbaea0f7eb9b5c7a9ff6f22382ea1f0bd21`. Baseline: main #79, `5af01a16508c6e38aa73644bb686d55f17bb9b88`. The evidence follow-up changes only reports.

## Method

Seven repeats; 1,000 identical deterministic logical path requests per engine, 10% misses. Both old/new binaries support native `cat_many`, so their grouped requests use the same frames and semantics. Existing individual cat lines are measured separately. Packed Git uses one persistent `cat-file --batch` process; Git gc and changed-path commit-graph preparation are outside query timing. Every byte, missing slot and frame is checked; engine order rotates.

The baseline initializes the shared fixture. N small binary fixture files plus one noise file, 11 commits. Sharded layout uses 256 leaf directories; nested layout uses 1,024 leaf directories beneath another 16-way level. Each timed query starts a new process and opens/checks the pack index again. OS caches are warm. These are small binary-file workloads, not a representative large-source/large-blob corpus.

Linux x86-64 container, Intel Xeon Platinum 8573C, nine reported CPUs with an eight-CPU cgroup quota, overlay filesystem; Rust 1.99.1 release builds. Both binaries execute on this host. These timings must not be compared directly with the earlier report from an AMD host. Git version: git version 2.51.1.

New binary SHA-256: `b170425f732cf6c23b9c28b4e5b16150b809bcf6362af382a29a107f66dc5f05`. Baseline/harness hashes and all raw samples are in both JSON files. CPU time, peak RSS, cold storage, network and multi-client throughput were not measured. Seven samples give descriptive, rough tail estimates.

## End-to-end grouped query p50 (milliseconds)

| Layout | Files | Previous grouped | New, no pack | New, explicit pack | Git batch |
| --- | ---: | ---: | ---: | ---: | ---: |
| sharded | 1,000 | 8.55 | 9.39 | 5.05 | 66.66 |
| sharded | 10,000 | 12.97 | 11.78 | 6.86 | 78.71 |
| sharded | 100,000 | 24.60 | 22.42 | 22.37 | 160.13 |
| nested | 10,000 | 14.24 | 11.77 | 7.91 | 38.90 |

| Layout | Files | Pack vs previous grouped | Pack vs same new binary, no pack | Pack vs Git batch |
| --- | ---: | ---: | ---: | ---: |
| sharded | 1,000 | 1.69× | 1.86× | 13.20× |
| sharded | 10,000 | 1.89× | 1.72× | 11.48× |
| sharded | 100,000 | 1.10× | 1.00× | 7.16× |
| nested | 10,000 | 1.80× | 1.49× | 4.92× |

At 100k files, pack and new unpacked medians are effectively equal in this sample. Loading/checking the full index costs time even when the group requests only a subset. The pack remains explicit; ordinary queries do not pay its index-memory or startup cost. The object-ID codec change also affects ordinary storage reads, with mixed measured outcomes below.

## Preparation and duplicated storage

| Layout | Files | Pack preparation ms | Additional pack bytes |
| --- | ---: | ---: | ---: |
| sharded | 1,000 | 32.76 | 173,748 |
| sharded | 10,000 | 158.24 | 1,487,748 |
| sharded | 100,000 | 1051.33 | 14,627,748 |
| nested | 10,000 | 128.20 | 1,571,636 |

Preparation is one observation per fixture, not a seven-sample median. It is excluded from query latency and retains all live objects. Including preparation makes the first group slower in every control; the optimization requires reusing the pack. Pack index bytes are separately exposed by `stats` and are additional to the object-cache budget. The index uses one retained buffer, capped at 48,000,000 bytes. No RSS reduction is claimed.

## Retained controls and regressions

Ratios above one mean faster new Loom. Single-command Git/Loom ratios below one mean Loom is slower. No control is removed because it misses the target.

| Layout | Files | Previous/new lines | Previous/new grouped, no pack | CLI cat Git/new | CLI files Git/new | Status Git/new |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharded | 1,000 | 0.94× | 0.91× | 0.58× | 0.76× | 0.62× |
| sharded | 10,000 | 1.04× | 1.10× | 0.91× | 0.90× | 0.20× |
| sharded | 100,000 | 1.20× | 1.10× | 1.03× | 1.18× | 0.13× |
| nested | 10,000 | 0.98× | 1.21× | 1.12× | 1.04× | 0.30× |

The 1k unpacked line/group controls regress in this sample; existing clients do not receive a universal speedup. Status still scans content and remains much slower than Git. CLI startup and host variability remain significant; do not extrapolate these ratios to other workloads.

## Correctness and compatibility

192 local tests pass, plus strict fmt and Clippy. Coverage includes pack roundtrips/deduplication, bounded malformed indexes, checksummed-index vs actual object corruption, a mutation after opening, missing live objects remaining errors in live storage, failed-build cleanup, existing output preservation, competing builders, Unix symlink/hardlink/FIFO/oversize rejection, SQLite, zero cache, revisions after preparation, query independence from the workspace writer lock, binary framing and strict CLI options. Object-ID text golden/all-byte checks retain lowercase output and either-case parsing; non-hex numeric aliases are now rejected by the existing hex-digit contract.

Live object bytes/IDs and database schemas are unchanged. Packs never satisfy fsck, publication, recovery or replica closure; they are disposable acceleration files, not backups. A corrupt selected object errors instead of silently falling back; absent IDs use the normal session. Existing destinations are never overwritten. Preparation can materialize derived native-tree metadata and uses normal repository recovery; queries remain read-only. No power-loss durability or production certification is inferred from the cache tests.

[Reproduction](../benchmarks/README.md#explicit-read-pack-comparison), [protocol/resources](snapshot-query-protocol.md#optional-immutable-read-packs), [format](read-pack-format.md), [raw sharded](../benchmarks/results/read-pack-sharded-20261016.json), [raw nested](../benchmarks/results/read-pack-nested-20261016.json).
