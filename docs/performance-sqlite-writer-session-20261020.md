# Operation-scoped SQLite writer session — 20 October 2026

This change lets one bounded `ObjectPublisher` retain a single SQLite write
connection across its publication windows. It removes repeated connection open,
busy-timeout setup and `synchronous=FULL` configuration from a multi-window
operation. It does **not** defer or combine commits: every window retains its
own `BEGIN IMMEDIATE`, conditional inserts, in-transaction byte verification,
WAL/FULL commit and independent fresh live readback before ref publication.

The generic capability is optional. Non-SQLite providers keep the existing
batch or scalar fallback. The SQLite session is operation-scoped rather than a
process-global pool, so separate CLI commands and concurrent processes still
open their own writers. It compares the configured database file identity
before and after each window; removal or replacement produces an error rather
than acknowledging writes through an old open handle.

## Correctness coverage

- one session is opened for a publisher spanning two 128-object windows;
- both windows retain fresh live readback and the direct/scalar paths stay unused;
- an injected SQL error rolls back its complete window and the same retained
  writer can retry idempotently;
- a mismatching database identity and, on Unix, an unlinked live database fail
  without publishing bytes to the replacement path;
- the existing collision, canonical-ID, replica, closure, staging, recovery and
  ref-CAS suites remain required gates.

## Measurement contract

The existing checked SQLite workflow builds exact base and head sources into
separate Cargo targets and records commit/tree, binary and harness hashes. Every
sample creates fresh worktree and object databases for candidate, previous Loom
and Git. Candidate and previous Loom receive identical native paths and explicit
`--scan-workers`; Git runs `add` plus `commit`. The timed commit ends only after
the normal durable object transactions, live verification and ref-CAS.

Scenarios cover 32 and 256 files, a serial 256-file control, and 2,048 files in
64 directories with 64 incremental changes. The report retains p50/p95/p99 wall
and CPU time, peak RSS, process I/O counters and logical/allocated storage. Read
workflows run as controls even though this write-session change is not expected
to improve them. OS caches are not controlled. Network, sync syscall counts,
WAL peak/backlog, recovery duration, cold-cache runs and 10/100/500-agent loads
remain unmeasured; no result from this run establishes a universal speedup.

## Exact implementation-head results

Run `39388355261` measured implementation head
`ad2175a9a70fa92e4aea5478e73eb83f5e175e0a` against exact base
`59274ba468f7d633cd77bd165d29e9fc27c00e1a`. Their trees were respectively
`f14f4e640941582776b71efff5e56fca6454472a` and
`948205856c9bdbb8f389ba518a411b260d2f481f`. Separate release binaries had
SHA-256 `10b1e370…42b6e8d` and `4db6b8d0…310716`; the harness was
`14585569…be46b`. The artifact is `sqlite-batch-ad2175a9…` (ID
`12249086124`). Both Loom binaries were actually started with identical native
directory inputs and the same explicit worker request shown below.

`Previous/candidate` greater than one favors this change. `Loom/Git` greater
than one means Git was faster; Git's different format and integrity contract
make it an external workflow baseline, not an engine-only comparison.

| files / directories / changed | workers | phase | previous p50 | candidate p50/p95/p99 | previous/candidate | Loom/Git p50 |
|---:|---:|---|---:|---:|---:|---:|
| 32 / 32 / 8 | 0 | initial | 91.249 ms | 91.699 / 95.527 / 96.078 ms | 0.995× | 6.04× slower |
| 32 / 32 / 8 | 0 | incremental | 56.735 ms | 55.373 / 58.857 / 59.351 ms | 1.025× | 6.26× slower |
| 256 / 32 / 8 | 0 | initial | 201.934 ms | 197.775 / 351.779 / 403.203 ms | 1.021× | 4.11× slower |
| 256 / 32 / 8 | 0 | incremental | 161.630 ms | 160.650 / 164.315 / 164.548 ms | 1.006× | 10.60× slower |
| 256 / 32 / 8 | 1 | initial | 204.866 ms | 199.281 / 201.629 / 201.995 ms | 1.028× | 3.97× slower |
| 256 / 32 / 8 | 1 | incremental | 163.249 ms | 165.055 / 166.502 / 166.745 ms | 0.989× | 10.19× slower |
| 2,048 / 64 / 64 | 0 | initial | 1,186.717 ms | 1,170.439 / 1,181.300 / 1,183.101 ms | 1.014× | 4.03× slower |
| 2,048 / 64 / 64 | 0 | incremental | 1,174.234 ms | 1,159.036 / 1,171.973 / 1,173.446 ms | 1.013× | 13.91× slower |

The 256-file automatic-worker initial case contains an unfavorable candidate
tail outlier: p95/p99 rose to 351.779/403.203 ms while the previous version was
204.613/205.234 ms. It must not be hidden by the 1.021× p50. The serial
incremental p50 also regressed 1.1%. The other named write p50 changes range
from a 0.5% regression to a 2.8% improvement; this is small evidence that open
connection work matters, not a broad speed claim.

Candidate/previous p50 CPU time was 58.230/57.947 ms for the 32-file initial
case, 165.221/169.936 ms for serial 256-file initial, and
1,106.863/1,126.302 ms for 2,048-file initial. Reported peak RSS was unchanged
within each paired scenario (about 25.2–27.0 MiB). Candidate/previous output-I/O
counters were 38,072/38,312 units for 256-file initial and
254,096/258,904 for 2,048-file initial; input counters were zero. Final logical
and allocated footprints were identical between Loom versions: 376,832 bytes
for 256 files and 2,064,384 bytes for 2,048 files. These process counters are
not physical byte or sync-syscall measurements.

Read controls changed by roughly ±1.1% at p50 and are not attributed to this
writer optimization. In the 2,048-file byte-query control Loom was 2.49× Git,
as before; Git remained far faster for both durable commit workflows and the
non-equivalent integrity/history/listing diagnostics. Nothing here proves 100×
Git, universal readiness, network behavior, crash safety or agent-scale load.
