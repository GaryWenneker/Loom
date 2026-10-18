# Fresh SQLite live readback — 2026-10-18

The 64-directory incremental workflow (2,048 files, 64 changes) records 4,805.57 → 885.57 ms: **5.43× previous Loom**. Git records 62.02 ms, about 14.28× faster than the candidate. The 256-file / eight-change case records 1,217.52 → 142.32 ms (8.56× previous Loom); Git records 31.38 ms. This is measured SQLite progress, not 100× Git or universal product performance.

## Implemented change

SQLite publication already wrote a bounded window in one durable transaction, but post-commit verification reopened the database per object. The core now requests one fresh live readback window, at most 128 objects / 8 MiB, with the existing one-oversized-object exception. SQLite prepares its SELECT once for that call; it also prepares INSERT and verification SELECT statements once inside the write transaction. One-off reads keep their direct preparation path. No statement cache, connection, payload or closure acknowledgment survives a live batch call.

External closure validation previously called SQLite `exists` (a verified read) followed by another verified `get`. SQLite's live-read contract now supplies both presence and bytes in one read. Core still validates response count, order, byte budget, hashes, envelopes and expected object kinds, traverses the full closure, and compares publication bytes exactly. Opt-in requires presence on every required replica; default/replicated backends retain the original all-copy existence plus get fallback. No retained read session can acknowledge publication. Empty publication flushes do no I/O.

WAL, `synchronous=FULL`, conditional creation, per-window rollback, canonical IDs, staging and ref CAS remain unchanged. A failed live readback can leave safe immutable objects, but does not acknowledge new staged state or a branch ref. Retry is idempotent. Fault regressions cover incomplete/extra/reordered/corrupt/over-budget/error responses, type mismatch, malformed hash-matching envelopes, missing copies, fresh database reads, lost databases, one large object and lossless staging/ref retry.

## Source and method

Measured candidate [8de720f4](https://github.com/GaryWenneker/Loom/commit/8de720f4c8173d2b2c110cede816aba72d37a2da), tree `a3eeabb7668a3222a42f081505aa8488c33b6f91`. Baseline main [93087efa](https://github.com/GaryWenneker/Loom/commit/93087efaa557beed930f32dc045f7d71fee9d778), tree `6766e5968019dd572518efbdfe9be6fcdcb4a57f`. Candidate binary SHA-256 `8bab775d65d126f1cbb38d4016fea025c07c59da4779b93f44b530bd404956b0`; baseline `5ef29b66d67b545aff32b64457b5e1d6eff6ff715187b7ad7389ac5eb67a2cba`. Both use Rust 1.99.1 and separate Cargo targets on the same host. The final source passed 276 local workspace/all-target tests, formatting and strict Clippy. Exact-head CI and the resulting-main checks are recorded in PR #107.

Every sample creates fresh byte-identical nested worktrees and an external SQLite object database. The timed initial commit publishes the full fixture; the incremental commit changes eight files or 64 files across 64 directories. Both native binaries receive the **same explicit worker request and the same complete command arguments**, except executable path; schema 2 retains the actually started arguments. Git measures external add plus commit. Fixtures/setup and full final payload, exact path listing, equal native root and complete history fsck checks are outside timing. No unmeasured publication precedes the timed initial native commit.

Automatic small cases use seven repeats; serial, wide and 64-KiB cases use five. Engine order rotates per repeat. These are separate worker/size runs, not an interleaved causal experiment of one scheduling policy. Fresh workspaces do not mean controlled cold OS caches. No local builds/tests overlapped the timed runs; other host scheduling and storage load remain uncontrolled. All unfavorable cases and the earlier prototype remain below and in raw data.

## Complete workflows, p50

| Both requests | Files | Bytes/file | Changed files | Phase | Previous Loom ms | New Loom ms | Git ms | Previous/new | Git/new |
| ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 0 | 32 | 256 | 8 | initial | 330.33 | 181.14 | 71.24 | 1.82× | 0.39× |
| 0 | 32 | 256 | 8 | incremental | 261.62 | 86.90 | 24.80 | 3.01× | 0.29× |
| 0 | 256 | 256 | 8 | initial | 1306.18 | 453.30 | 222.79 | 2.88× | 0.49× |
| 0 | 256 | 256 | 8 | incremental | 1217.52 | 142.32 | 31.38 | 8.56× | 0.22× |
| 1 | 256 | 256 | 8 | initial | 1294.46 | 434.80 | 213.61 | 2.98× | 0.49× |
| 1 | 256 | 256 | 8 | incremental | 1220.17 | 144.80 | 31.63 | 8.43× | 0.22× |
| 0 | 2,048 | 256 | 64 | initial | 6869.16 | 2344.15 | 875.08 | 2.93× | 0.37× |
| 0 | 2,048 | 256 | 64 | incremental | 4805.57 | 885.57 | 62.02 | 5.43× | 0.07× |
| 0 | 128 | 65,536 | 64 | initial | 390.82 | 303.95 | 218.77 | 1.29× | 0.72× |
| 0 | 128 | 65,536 | 64 | incremental | 421.75 | 331.78 | 157.38 | 1.27× | 0.47× |

Ratios above one favor the new candidate. These are whole-workflow comparisons, not isolated proof of one function's cost. They do not establish universal improvement or 100× Git. Git's default transaction and power-loss contracts differ from Loom's live-verified SQLite workflow; its figures describe external user work, not identical database operations.

## CPU and raw RSS, p50

| Both requests | Files | Bytes/file | Changed files | Phase | Previous CPU ms | New CPU ms | Git CPU ms | Previous raw RSS MiB | New raw RSS MiB | Git raw RSS MiB |
| ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 32 | 256 | 8 | initial | 268.12 | 125.73 | 69.62 | 20.25 | 20.25 | 20.25 |
| 0 | 32 | 256 | 8 | incremental | 238.43 | 63.92 | 23.09 | 20.25 | 20.25 | 20.25 |
| 0 | 256 | 256 | 8 | initial | 1221.33 | 385.77 | 219.73 | 20.31 | 20.31 | 20.31 |
| 0 | 256 | 256 | 8 | incremental | 1167.45 | 120.33 | 29.46 | 20.31 | 20.31 | 20.31 |
| 1 | 256 | 256 | 8 | initial | 1231.47 | 368.13 | 211.47 | 20.25 | 20.25 | 20.25 |
| 1 | 256 | 256 | 8 | incremental | 1181.32 | 122.31 | 30.26 | 20.25 | 20.25 | 20.25 |
| 0 | 2,048 | 256 | 64 | initial | 6662.54 | 2199.53 | 866.50 | 21.48 | 21.48 | 21.48 |
| 0 | 2,048 | 256 | 64 | incremental | 4631.23 | 775.66 | 58.63 | 21.48 | 21.48 | 21.48 |
| 0 | 128 | 65,536 | 64 | initial | 287.92 | 198.85 | 216.36 | 37.93 | 37.93 | 37.93 |
| 0 | 128 | 65,536 | 64 | incremental | 334.03 | 220.24 | 155.91 | 37.93 | 37.93 | 37.93 |

Raw files preserve each sample and p50/p95/p99 wall/CPU/RSS/I/O, binary/harness identities, native root sequences and logical/allocated metadata bytes. Maximum RSS is raw wait4 process data with a possible launcher floor, not isolated engine memory or a total RSS cap. Kernel block counters are platform units, not portable bytes. Footprints describe the completed closed workflow; they do not measure peak live WAL/SHM or temporary disk use. Readback buffers add up to one window to publication buffers; path/history metadata and backend caches are additional. Five/seven samples give descriptive quantiles, not production tail SLOs.

## Earlier prototype retained

The first prototype [50a8f622](https://github.com/GaryWenneker/Loom/commit/50a8f6226993dfa7e0ab6e64760d64051bc3a9f5) still performed duplicate closure reads and used a statement cache even on one-off scalar reads. It showed mixed/regressing small workflows and was replaced. The following complete measurements and binary/source provenance are retained, not attributed to the final implementation. The prototype and final runs are separate rounds on a shared host; dividing their medians would not isolate a causal statement-cache effect.

| Both requests | Files | Bytes/file | Changed files | Phase | Previous Loom ms | Prototype ms | Git ms | Previous/prototype | Git/prototype |
| ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 0 | 32 | 256 | 8 | initial | 159.78 | 147.03 | 26.14 | 1.09× | 0.18× |
| 0 | 32 | 256 | 8 | incremental | 98.33 | 121.70 | 17.88 | 0.81× | 0.15× |
| 0 | 256 | 256 | 8 | initial | 624.63 | 571.58 | 113.60 | 1.09× | 0.20× |
| 0 | 256 | 256 | 8 | incremental | 693.65 | 646.03 | 19.10 | 1.07× | 0.03× |
| 1 | 256 | 256 | 8 | initial | 805.72 | 944.55 | 192.12 | 0.85× | 0.20× |
| 1 | 256 | 256 | 8 | incremental | 261.32 | 543.45 | 34.28 | 0.48× | 0.06× |
| 0 | 2,048 | 256 | 64 | initial | 7461.71 | 5756.10 | 854.90 | 1.30× | 0.15× |
| 0 | 2,048 | 256 | 64 | incremental | 6996.85 | 7380.34 | 65.30 | 0.95× | 0.01× |
| 0 | 128 | 65,536 | 64 | initial | 407.90 | 396.37 | 126.90 | 1.03× | 0.32× |
| 0 | 128 | 65,536 | 64 | incremental | 425.19 | 497.61 | 97.75 | 0.85× | 0.20× |

## Remaining work and reproduction

```sh
python3 benchmarks/sqlite_batch_publication.py \
  --loom /path/to/candidate/loom --baseline-loom /path/to/baseline/loom \
  --files 32 256 --repeat 7 --scan-workers 0 --output automatic.json
python3 benchmarks/sqlite_batch_publication.py \
  --loom /path/to/candidate/loom --baseline-loom /path/to/baseline/loom \
  --files 2048 --directories 64 --changed-files 64 --repeat 5 \
  --scan-workers 0 --output multidir.json
```

Use `--files 256 --repeat 5 --scan-workers 1` for the serial control and `--files 128 --file-bytes 65536 --directories 64 --changed-files 64 --repeat 5 --scan-workers 0` for the larger-payload case. Both native engines receive the worker request. Draft heavy jobs defer until ready-for-review; all relevant final exact-head jobs and all five resulting-main checks remain required before the integration is declared complete.

Loom remains experimental. Secure remote authority/transport, sparse workspace lifecycle, full native-v2 migration, broad crash/soak and concurrent-agent evidence remain open under #13/#26. This local SQLite optimization neither completes those release gates nor establishes the 100× Git goal.

[Contract](sqlite-batch-publication.md). [Source/compiler/binary/raw provenance](../benchmarks/results/sqlite-live-read-provenance-20261018.json). Final raw data: [automatic](../benchmarks/results/sqlite-live-read-auto-20261018.json), [serial](../benchmarks/results/sqlite-live-read-serial-20261018.json), [64 directories](../benchmarks/results/sqlite-live-read-multidir-20261018.json), [64-KiB payloads](../benchmarks/results/sqlite-live-read-payload-20261018.json). Prototype raw data use the same names with `prototype-` before the scenario label.
