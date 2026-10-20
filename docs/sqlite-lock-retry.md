# Bounded SQLite lock retries

Loom uses SQLite WAL mode with `synchronous=FULL` for durable canonical object
publication. On Windows, concurrent WAL writers can exceptionally return
`SQLITE_PROTOCOL` while starting a write transaction. SQLite documents this as
a locking-protocol race that can occur when many connections compete to become
the writer; it is distinct from corruption.

The object backend treats `SQLITE_BUSY`, `SQLITE_LOCKED`, and
`SQLITE_PROTOCOL` as transient only while acquiring `BEGIN IMMEDIATE` and while
running the idempotent database bootstrap. Retries use a 10 ms delay. The
bootstrap has one five-second deadline; transaction acquisition starts one
five-second retry window only after SQLite returns the first retryable failure.
This distinction matters because SQLite can spend time in its own busy/protocol
handling before returning. Once a transaction begins, statement or commit
failures are returned without replay. This avoids retrying an ambiguous durable
commit.

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
