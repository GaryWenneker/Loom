# Bounded SQLite lock retries

Loom uses SQLite WAL mode with `synchronous=FULL` for durable canonical object
publication. On Windows, concurrent WAL writers can exceptionally return
`SQLITE_PROTOCOL` while starting a write transaction. SQLite documents this as
a locking-protocol race that can occur when many connections compete to become
the writer; it is distinct from corruption.

The object backend treats `SQLITE_BUSY`, `SQLITE_LOCKED`, and
`SQLITE_PROTOCOL` as transient only while acquiring `BEGIN IMMEDIATE` and while
running the idempotent database bootstrap. Retries use a 10 ms delay. Each
bootstrap or transaction acquisition starts one five-second retry window only
after SQLite returns the first retryable failure. This distinction matters
because SQLite can spend time in its own busy/protocol handling before
returning. Once a transaction begins, statement or commit failures are returned
without replay. This avoids retrying an ambiguous durable commit.

Within one process, Loom also hashes each canonical database path onto one of
64 fixed write gates. A gate covers bootstrap or the complete object
transaction, preventing same-database threads from repeatedly entering
SQLite's single-writer election together. The fixed stripe table cannot grow
with repositories or agents. Independent databases normally use different
stripes; a hash collision can conservatively serialize two databases but
cannot change their contents. Separate processes still coordinate through
SQLite WAL locks and the bounded retry policy above.

The policy does not change:

- WAL mode or `synchronous=FULL`;
- canonical object validation and conditional creation;
- transaction rollback, independent live readback, or replica verification;
- closure verification or atomic ref compare-and-swap;
- the existing database identity and replacement checks.

CI repeats both concurrent initialization and concurrent immutable creation ten
times on the exact Windows PR head. The SQLite workflow also compares complete
durable commit, query, and `fsck` workflows with the exact previous Loom binary
and Git. Lock retries are a correctness fallback, not a throughput claim;
uncontended paths perform no sleep and allocate no retry queue.
