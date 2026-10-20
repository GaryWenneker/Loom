# Atomic SQLite object batches

Loom's generic object publisher buffers at most 128 canonical objects or 8 MiB
before flushing. SQLite now consumes each such window in one `BEGIN IMMEDIATE`
transaction instead of opening and committing one transaction per object.
For one `ObjectPublisher`, SQLite now also retains one write connection across
successive bounded windows. Every window still starts and commits its own
`BEGIN IMMEDIATE` transaction with WAL and `synchronous=FULL`; the session does
not combine durability boundaries or let buffers grow past the existing limits.

The optimization does not change object IDs or ref publication. Every envelope
and hash is validated before the transaction starts. Inserts retain
`INSERT OR IGNORE` conditional-create semantics, and every resulting row is read
and hash-checked inside the transaction before commit. Any SQL error, missing or
corrupt existing row, or immutable collision rolls the complete new window back.
After commit, `ObjectPublisher` performs a fresh live read of every object before
the repository may publish a tree, changeset or compare-and-swap ref update.
Those reads use independent read-only connections and never trust the retained
writer. The writer checks the database file identity before and after every
window and fails closed if the configured path disappears or is replaced.

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
Across cloned handles to one `ObjectStore`, publishers now reserve both their
canonical envelopes and the maximum independent readback allocation from one
shared budget. The default is twice the maximum v1 canonical-object size, so a
single valid large object remains supported while competing managed publishers
receive backpressure. Reservations are returned after every verified window and
when an unfinished or failed publisher is dropped. Caller-owned inputs,
SQLite/page caches, allocator overhead and separately opened stores are not
covered, so this remains a managed publication bound rather than a total RSS cap.
The database path is rechecked before return; Unix also compares device/inode,
and Windows checks creation time. These observations do not constitute a lease
or protection from every concurrent filesystem substitution. Ordinary scalar
reads and authoritative enumeration now open fresh `READ_ONLY`,
`query_only=ON` connections. They no longer request a write-capable handle or
reapply `synchronous=FULL` merely to execute `SELECT`; every returned object is
still hash- and envelope-verified. Missing storage is never recreated by reads;
missing schemas, corrupt bytes and missing requested rows fail closed. These
checks do not pin the lifetime identity of a valid replacement database. Write transactions retain WAL plus
`synchronous=FULL`. Live closure verification and fsck retain their normal
contracts.

Typed external-closure verification also reuses this live-read contract for
one object at a time. For SQLite, one validated live read proves both existence
and canonical bytes; it replaces the previous `exists` read followed by another
`get` of the same row. The core still verifies the expected object kind and
recursively visits the complete closure. A provider may opt in only if its live
read proves presence on every required copy. Replicated providers keep their
default `None` implementation and therefore the original all-replica existence
check plus verified read. No cross-call existence or closure cache is added.

Retries are idempotent: an already committed batch verifies the same immutable
rows without replacing them. SQLite remains in WAL mode with
`synchronous=FULL`; no sync or durability control is disabled for performance.
Providers that do not implement atomic batches retain the existing bounded
scalar/parallel fallback. The generic session capability is optional and scoped
to one publisher; providers without it preserve their previous behavior.

Retained immutable-query sessions reuse one parsed SELECT, not object payloads
or an open read transaction. Every read still checks the row length, canonical
envelope and BLAKE3 ID. Publication, closure verification and fsck continue to
use fresh live storage rather than those retained sessions.

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
After both commits, it separately measures and validates one complete all-file
batch query and one full integrity check. The previous Loom executable receives
identical paths, worker requests and read/integrity commands. Git is an
external baseline using `add` plus `commit`; its storage transaction contract is
not identical, so the comparison describes end-to-end user work rather than
equivalent internal database operations.

Use `--directories 64 --changed-files 64 --files 2048` for a wider incremental
case, or pair `--scan-workers 1` on both native engines for a serial control.
Schema 2 records the actual measured arguments, file size, directory count and
explicit worker request. Every repeat starts fresh repositories and measures initial publication and
the defined incremental commit.

Fixture creation, path listing, canonical Loom root comparison and the repeated
final verification occur outside timing. The first complete batch query and
`fsck` after both commits are timed and checked. The report retains p50/p95/p99 wall/CPU, peak
RSS, POSIX block-I/O counters, and logical/allocated metadata bytes. It does not
control cold caches or measure network, remote caches, multi-agent contention or
power-loss recovery.

With `--read-workflows`, schema 3 additionally times complete `fsck`, history,
listing and batch-content commands after the two commits. Each runs in a new
process in that order. Batch queries read every file twice with Loom's object
cache disabled, and record input digests and request counts. Listing can populate
the derived historical index before the query; these are not cold-query results.
Output parsing, byte comparison and independent final verification remain
outside timing. Git history and fsck are diagnostic comparisons because the
formats and integrity obligations differ.

[Measured complete workflows and retained prototype results](performance-sqlite-live-read-20261018.md).

[Isolated parsed-statement comparison and retained combined measurements](performance-sqlite-statements-20261018.md).
