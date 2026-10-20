# Bounded SQLite scalar read reuse

Ordinary SQLite object `get`, `exists` and `list_ids` operations now lazily
share one read-only connection per backend instance. This removes repeated
connection setup from complete integrity scans and other scalar-read-heavy
workflows without creating an unbounded connection pool.

The retained connection is opened with SQLite's read-only and no-mutex flags,
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

CI compares full commits, `fsck`, history, listing and cache-disabled query
workflows with the exact previous Loom binary and Git. Candidate and baseline
use separate targets and identical native arguments. Cold-cache, network and
power-loss evidence must be reported separately rather than inferred from this
local warm-cache workflow.
