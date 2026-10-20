# Bounded SQLite scalar read reuse

Ordinary SQLite object `get`, `exists` and `list_ids` operations now lazily
reuse one read-only connection per backend instance. A caller takes that reader
only when it is immediately available. Independent concurrent callers never
wait behind it: they open a temporary read-only connection whose lifetime is
bounded by that call. Existing operation worker limits therefore bound total
concurrency without adding another wait cycle.

Every connection is opened with SQLite's read-only and no-mutex flags,
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

The first benchmarked blocking single-reader candidate was rejected because it
made parallel history and file listing about 3.8 times slower in the 2,048-file,
64-directory case. A subsequent fixed-capacity waiting pool was also rejected:
a complete commit timed out after ten minutes, exposing a wait cycle. Both
results remain attached to superseded PR heads and are not inherited by the
non-blocking design.

CI compares full commits, `fsck`, history, listing and cache-disabled query
workflows with the exact previous Loom binary and Git. Candidate and baseline
use separate targets and identical native arguments. Cold-cache, network and
power-loss evidence must be reported separately rather than inferred from this
local warm-cache workflow.
