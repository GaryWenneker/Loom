# Bounded SQLite scalar read reuse

Ordinary SQLite object `get`, `exists` and `list_ids` operations now lazily
share a pool of at most eight read-only connections per backend instance. This
removes repeated connection setup from complete integrity scans and other
scalar-read-heavy workflows without serializing independent callers or creating
an unbounded connection pool. Connections are opened only on demand; callers
wait with backpressure after the fixed limit is reached.

Each retained connection is opened with SQLite's read-only and no-mutex flags,
uses `query_only=ON`, and never starts an explicit long-lived transaction.
Every statement therefore completes before the next lookup and can observe
later committed immutable objects. Prepared statements are reused only through
SQLite's bounded per-connection statement cache.

The backend records the database file identity when it opens and checks that
identity before and after every pooled operation. A removed, symlinked or
replaced database fails closed instead of switching the backend to unrelated
storage. Returned envelopes still undergo hash and canonical-format validation.

This pool is not used for publication acknowledgment. Durable writes retain
WAL and `synchronous=FULL`; every publication window still receives a fresh
independent live readback before closure verification and ref compare-and-swap.
Explicit query snapshots may continue to open their own bounded read sessions
for parallel immutable work.

The first benchmarked single-reader candidate was rejected because it made
parallel history and file listing about 3.8 times slower in the 2,048-file,
64-directory case. Its raw artifact remains attached to the superseded PR head;
the bounded pool must be remeasured rather than inheriting those results.

CI compares full commits, `fsck`, history, listing and cache-disabled query
workflows with the exact previous Loom binary and Git. Candidate and baseline
use separate targets and identical native arguments. Cold-cache, network and
power-loss evidence must be reported separately rather than inferred from this
local warm-cache workflow.
