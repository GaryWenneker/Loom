# SQLite immutable-reader statement reuse — 18 October 2026

PR #110 reuses the parsed SELECT in a retained immutable SQLite read session. Every invocation still reads current rows and verifies length, BLAKE3 ID and canonical envelope. Rows and parameters are reset between calls. There is no object-payload cache or retained read transaction in this change. Fresh live publication, closure and fsck reads remain separate; write durability remains WAL/FULL.

PR #109 first changed scalar reads/enumeration to fresh read-only connections. It was merged as `aef258a5c6d1f04e94d6509f6a67cd93acbad206`, tree `16a75e3206cff57cefabebe3593ce31ec8fe17f3`, after six green PR jobs. All five resulting-main checks passed in run [39105414641](https://github.com/GaryWenneker/Loom/actions/runs/39105414641). Its independent CI measured roughly 1.21–1.23× full-integrity speedup, with nearly unchanged queries; it is not part of the isolated factors below.

## Isolated local comparison

Candidate source `97a132b4980f99763a70c537467a972f199f9e92`, tree `ea342d4a1c8c37ce94bf1fcec5766e508d7a51a8`, is compared with the exact #109 merge above. Separate locked release builds use Rust 1.99.1. The source differs in statement reuse plus a regression; benchmark and documentation changes add no runtime behavior. Local fmt, strict Clippy and 280 workspace/all-target tests pass, including the newly named cached-reader regression. Exact-head CI and final integration evidence are recorded on [PR #110](https://github.com/GaryWenneker/Loom/pull/110).

All values below are local shared-host observations, not guarantees. Factors divide medians; values below 1 retain regressions. Git is a byte-read baseline for query and a diagnostic for fsck/history, whose object formats and verification obligations differ.

| Case | Operation | Previous Loom p50 ms | Candidate p50 ms | Git p50 ms | Previous / candidate | Git / candidate |
|---|---|---:|---:|---:|---:|---:|
| 32 files / 32 dirs / w0 | query | 3.962 | 3.087 | 4.058 | 1.283× | 1.314× |
| 32 files / 32 dirs / w0 | fsck | 53.093 | 47.363 | 6.951 | 1.121× | 0.147× |
| 256 files / 32 dirs / w0 | query | 15.619 | 11.160 | 24.051 | 1.400× | 2.155× |
| 256 files / 32 dirs / w0 | fsck | 204.023 | 209.919 | 10.105 | 0.972× | 0.048× |
| 256 files / 32 dirs / w1 | query | 15.794 | 12.045 | 22.305 | 1.311× | 1.852× |
| 256 files / 32 dirs / w1 | fsck | 196.757 | 219.735 | 10.908 | 0.895× | 0.050× |
| 2048 files / 64 dirs / w0 | query | 137.519 | 96.739 | 216.405 | 1.422× | 2.237× |
| 2048 files / 64 dirs / w0 | fsck | 1633.477 | 1627.985 | 31.303 | 1.003× | 0.019× |
| 128 files / 64 dirs / w0 / 64 KiB | query | 22.584 | 22.247 | 27.082 | 1.015× | 1.217× |
| 128 files / 64 dirs / w0 / 64 KiB | fsck | 229.145 | 231.647 | 43.366 | 0.989× | 0.187× |

The same runs retain complete commit, history and listing controls:

| Case | Initial previous / candidate | Incremental | History | Listing |
|---|---:|---:|---:|---:|
| 32 files / w0 | 1.046× | 1.062× | 1.042× | 1.026× |
| 256 files / w0 | 0.986× | 0.933× | 0.978× | 1.005× |
| 256 files / w1 | 1.138× | 1.093× | 1.018× | 0.906× |
| 2048 files / w0 | 1.079× | 1.023× | 0.935× | 0.987× |
| 128 files / w0 / 64 KiB | 0.914× | 1.086× | 0.934× | 1.106× |

Full raw reports also retain p50/p95/p99 and individual wall/CPU/RSS/block-I/O samples, actual argv, query input digests, final snapshot roots and logical/allocated metadata footprint. RSS is the raw wait4 high-water value and can include a launcher floor; it is not an isolated engine-memory bound. Footprints are measured after the commands, not peak WAL/SHM or peak disk usage.

## Method and limits

- Every engine/sample starts with a fresh worktree and external SQLite database, publishes the same initial and incremental bytes, and uses the same native directory paths and explicit scan-worker request. Git runs external add+commit. Engine order rotates; timing excludes fixture creation and final repeated byte/listing/fsck checks.
- Extended read commands each start a new process after both commits, in fsck/history/files/query order. Query input is prepared outside timing. Every file is read twice with Loom object cache disabled (`--cache-bytes 0`); Git receives the same two-pass file-content requests in its own protocol. Every output byte is parsed and compared. Listing can populate the derived historical index before query, so this is not cold-query evidence.
- Both Loom engines receive identical actual arguments after binary and changeset-ID normalization. Their query stdin digests match exactly. Final snapshot roots agree per repeat; all history is structurally checked by fsck. This does not claim pairwise equality of every historical root or changeset ID.
- Auto uses 32/256 files, 32 directories, eight incremental changes and seven repeats. Serial uses 256 files, worker 1 and five repeats. Wide uses 2,048 files, 64 directories, 64 changes and five repeats. Payload uses 128 files of 64 KiB, 64 directories/changes and five repeats. Each file has distinct versioned content.
- The unchanged native worker algorithms and requested worker settings are controls, not a new sequential-versus-parallel implementation. Host scheduling and OS caches are uncontrolled; no builds/tests overlap local timed runs. No controlled cold-cache, network, cache-hit-rate, remote-cache, multi-agent, crash/soak or power-loss result is supplied.
- Missing storage is not recreated. The regression verifies uncommitted/committed updates, corruption and repair after errors, newly published rows and deletion visibility through the same cached statement. This is not a filesystem lifetime lease and does not reject every valid database substitution.
- No universal 100× Git result or release completion is established. Issues #13 and #26 remain open.

## Retained combined pre-integration results

The first measurements used candidate `668e049e3f61c026b67921d4c7aa2fe2237bd622` against pre-#109 base `c13e3bafb6fdbbc833a4d76b27df5c7a024e4662`. They combine scalar read-only connections and statement reuse. All four complete reports remain unchanged under `sqlite-statement-combined-*`; they are not isolated statement-reuse results. Their larger fsck gains must not be attributed to parsed-statement caching, and unpaired factors must not be multiplied to imply a speedup. The different source/harness hashes are recorded independently.

## Raw data

- [sqlite-statement-isolated-auto-20261018.json](../benchmarks/results/sqlite-statement-isolated-auto-20261018.json)
- [sqlite-statement-isolated-serial-20261018.json](../benchmarks/results/sqlite-statement-isolated-serial-20261018.json)
- [sqlite-statement-isolated-multidir-20261018.json](../benchmarks/results/sqlite-statement-isolated-multidir-20261018.json)
- [sqlite-statement-isolated-payload-20261018.json](../benchmarks/results/sqlite-statement-isolated-payload-20261018.json)
- [sqlite-statement-combined-auto-20261018.json](../benchmarks/results/sqlite-statement-combined-auto-20261018.json)
- [sqlite-statement-combined-serial-20261018.json](../benchmarks/results/sqlite-statement-combined-serial-20261018.json)
- [sqlite-statement-combined-multidir-20261018.json](../benchmarks/results/sqlite-statement-combined-multidir-20261018.json)
- [sqlite-statement-combined-payload-20261018.json](../benchmarks/results/sqlite-statement-combined-payload-20261018.json)
- [sqlite-statement-provenance-20261018.json](../benchmarks/results/sqlite-statement-provenance-20261018.json)
