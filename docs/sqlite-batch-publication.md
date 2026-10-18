# Atomic SQLite object batches

Loom's generic object publisher buffers at most 128 canonical objects or 8 MiB
before flushing. SQLite now consumes each such window in one `BEGIN IMMEDIATE`
transaction instead of opening and committing one transaction per object.

The optimization does not change object IDs or ref publication. Every envelope
and hash is validated before the transaction starts. Inserts retain
`INSERT OR IGNORE` conditional-create semantics, and every resulting row is read
and hash-checked inside the transaction before commit. Any SQL error, missing or
corrupt existing row, or immutable collision rolls the complete new window back.
After commit, `ObjectPublisher` performs a fresh live read of every object before
the repository may publish a tree, changeset or compare-and-swap ref update.

SQLite now opens one fresh, read-only connection per live readback window,
instead of opening a new connection for each object. Prepared SELECT and INSERT
statements are reused only inside the current connection/transaction. The live
batch API is separate from retained `ObjectReadSession`: no connection, snapshot
or payload cache survives a batch call. Each SELECT completes before the next,
and the core independently checks response count, input order, hash, envelope
and exact canonical bytes before acknowledging publication. Unsupported
providers retain the scalar live-read fallback; errors never request fallback.

The readback budget is the deduplicated publication window's exact encoded byte
total: at most 128 objects and 8 MiB, or one existing-format oversized object.
SQLite checks row lengths against the remaining budget before loading payloads.
Readback buffers are additional to the publication buffers, not a total RSS cap.
The database path is rechecked before return; Unix also compares device/inode,
and Windows checks creation time. These observations do not constitute a lease
or protection from every concurrent filesystem substitution. Existing scalar
reads, live closure verification and fsck retain their normal contracts.

Retries are idempotent: an already committed batch verifies the same immutable
rows without replacing them. SQLite remains in WAL mode with
`synchronous=FULL`; no sync or durability control is disabled for performance.
Providers that do not implement atomic batches retain the existing bounded
scalar/parallel fallback.

## Reproduce the comparison

```sh
python3 benchmarks/sqlite_batch_publication.py \
  --loom target/candidate/release/loom \
  --baseline-loom target/baseline/release/loom \
  --files 32 256 --repeat 7 \
  --scan-workers 0 \
  --output sqlite-batch-publication.json
```

Each sample creates a fresh external SQLite object database and worktree. It
measures a complete initial commit followed by an eight-file incremental commit.
The previous Loom executable receives the identical SQLite workload. Git is an
external baseline using `add` plus `commit`; its storage transaction contract is
not identical, so the comparison describes end-to-end user work rather than
equivalent internal database operations.

Use `--directories 64 --changed-files 64 --files 2048` for a wider incremental
case, or pair `--scan-workers 1` on both native engines for a serial control.
Schema 2 records the actual measured arguments, file size, directory count and
explicit worker request. Every repeat starts fresh repositories and measures initial publication and
the defined incremental commit.

Fixture creation, full byte reads, path listing, canonical Loom root comparison
and `fsck` occur outside timing. The report retains p50/p95/p99 wall/CPU, peak
RSS, POSIX block-I/O counters, and logical/allocated metadata bytes. It does not
control cold caches or measure network, remote caches, multi-agent contention or
power-loss recovery.
