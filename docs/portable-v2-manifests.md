# Portable v2 blob manifests

Loom can publish an additive `PortableBlobManifest` without changing v1
history or repository format selection:

```sh
loom portable-manifest regular path/to/file
loom portable-manifest executable path/to/tool
loom portable-manifest symlink path/to/link
loom portable-tree path/to/directory --workers 0
loom portable-export @TREE_ID new-workspace --workers 0
loom portable-verify @TREE_ID new-workspace --workers 0
loom portable-migrate-plan @CHANGESET_ID --workers 0
loom portable-migrate-publish @CHANGESET_ID --workers 0
loom portable-migrate-checkpoint @CHANGESET_ID --workers 0
loom portable-migrate-verify @CHANGESET_ID --workers 0
```

The object records regular, executable or symbolic-link identity, the exact
logical byte length and an ordered list of canonical content chunks. Chunk size
is fixed at 4 MiB and is part of the canonical schema. The v2 envelope kind is
part of the BLAKE3 input, separating manifest IDs from v1 blobs, changesets and
v2 trees. Repeating the same publication returns the same ID.

Before acknowledging a manifest Loom reopens it from live storage, verifies its
envelope and ID, reads every referenced chunk, verifies every chunk as a v1 blob
and checks exact sizes. `loom fsck` applies the same closure validation.
Publication does not update HEAD, a branch ref, staging, the worktree or the
revision-to-tree index. Failed publication can leave unused immutable chunks,
which are safe for idempotent retry and require future garbage collection.

Portable symlink targets are bounded to 4096 bytes and must be nonempty,
relative UTF-8 paths without backslashes, NUL bytes or parent traversal. This is
a deliberately narrow cross-platform contract, not silent pass-through of host
filesystem semantics.

`portable-tree` recursively publishes typed manifests and canonical directory
nodes. Entries are byte-sorted UTF-8 names and reference either a blob manifest
or another portable directory. Empty directories are retained. Names with
ASCII case collisions, Windows device aliases, control characters, trailing
dots/spaces, separators or repository metadata aliases are rejected rather
than producing a snapshot that changes identity across common hosts. Non-ASCII
Unicode normalization and case-folding remain explicit migration gates. The
command rejects special files and never follows symlinks.

Regular and executable files are read from an opened handle in fixed 4 MiB
chunks. Loom checks the declared handle length exactly, bounds the publication
window, and publishes the manifest only after all chunks are durable and live
verified. It then rechecks the opened handle and path metadata; on Unix the
device and inode must also remain identical. A failed or concurrently changed
input may leave unreachable immutable chunks, but never a manifest that claims
the incomplete stream. The 64 MiB v1 object limit therefore no longer limits
this additive command; the canonical manifest bound is 16,384 chunks (64 GiB).

For each directory, `--workers N` prepares independent regular files in
input-order-preserving worker batches. Zero automatically selects at most four
workers; explicit requests are bounded by available CPUs and the existing
eight-worker hard cap. Each preparation window is limited to 128 files and 8
MiB of input. Files larger than that retain the one-chunk-at-a-time streaming
path. Workers only read, hash and encode immutable data: durable chunk and
manifest publication, live closure verification and canonical parent-tree
publication occur after every worker has joined. This prevents a worker from
sharing repository SQLite state and provides backpressure independent of tree
size.

`portable-export` is the inverse materialization boundary for an immutable
portable tree. Loom live-verifies the complete manifest/chunk closure before it
creates the destination and refuses to reuse an existing path. It then creates
directories serially and streams independent files through provider-aware
workers: zero requests the existing adaptive policy, explicit requests remain
bounded by available CPUs, the eight-worker hard cap and the storage read
session's concurrency limit. Each worker holds at most one canonical 4 MiB
chunk at a time. Regular files are mode 0644 and executable files mode 0755 on
Unix. Safe relative symlinks use the host symlink API; hosts that cannot create
one fail the export instead of silently copying or rewriting it.

Every regular file is opened with exclusive creation and synced after its
verified bytes and mode are written. Directories are synced on Unix. Only after
the complete tree succeeds does Loom sync `.loom-portable-complete`, recording
the root ID and counts, and sync the destination directory. A failed attempt
may therefore leave an incomplete new directory without a receipt; retry must
use another absent destination. Export never moves a ref, updates staging,
changes history or touches the source worktree. The receipt is an explicit
completion signal, not a signature or remote authorization proof.

`portable-verify` independently reopens the canonical object closure and the
materialized workspace. It requires the exact expected directory members,
regular-file bytes, Unix executable classification where available, safe
symlink targets and byte-exact completion receipt. Independent files are
checked with the same provider-aware worker limits as export. A worker retains
one canonical chunk plus a 64 KiB comparison buffer; file identity and
directory membership are checked again after reading to reject observed
concurrent changes. Verification is read-only and never repairs or silently
accepts a partial workspace. This detects stable corruption and changes
observed during the pass; it is not an OS-level lease preventing an adversary
from changing a workspace immediately after successful verification.

`portable-migrate-plan` is the first explicit v1 migration preflight. It
replays the selected immutable first-parent history, live-verifies every
referenced v1 blob, rechunks bytes at the canonical 4 MiB boundary, validates
every portable directory name and computes the exact manifest/tree IDs a later
publisher must produce. Repeated one-, explicit multi- and adaptive-worker
runs return the same root. Workers are bounded by CPU, the eight-worker cap and
the provider read-session limit; because a v1 blob is at most 64 MiB, the hard
upper bound is eight decoded v1 blobs in flight. Duplicate blob content is
prepared once while logical byte totals still count every tracked path.

Planning is deliberately read-only: it publishes no chunks, manifests, trees,
derived indexes, refs, stage rows, receipts or worktree files. V1 never stored
historic executable bits, symlinks or empty directories, so the preflight
labels every tracked file regular and cannot recreate absent metadata.

`portable-migrate-publish` first performs that complete preflight, then prepares
unique source blobs in input-byte-bounded windows and feeds canonical chunks,
manifests and child-first trees into one bounded publisher. SQLite-backed
repositories conditionally create each publisher window in one durable
transaction; every committed object is read back and verified. The final root
is live-verified recursively. Repeating the command publishes no new object
identities and returns the same root for serial, explicit multi-worker and
adaptive runs.

This is deliberately additive publication, not a format switch. No ref, native
index, stage row, worktree file or repository version marker changes. Failure
before preflight completion publishes nothing; a later I/O failure can leave
only unreachable immutable objects from earlier durable windows, so retry is
safe.

`portable-migrate-checkpoint` adds a narrow durable prepared-state boundary.
It runs the same publication path, recursively live-verifies the root again at
the receipt boundary, then conditionally inserts an immutable source revision,
portable root and exact closure counts into the repository's `synchronous=FULL`
SQLite metadata transaction. An identical retry is idempotent. If that source
revision already has any different receipt value, the transaction fails closed
and never overwrites it. A crash before the receipt commit leaves only safe
additive objects and retry recomputes the same state; a committed receipt means
the closure was complete and verified when recorded.

`portable-migrate-verify` is the read-only recovery-side check for that
prepared state. It does not trust the SQLite row: it replays the immutable v1
revision, live-reads every referenced v1 blob, recomputes the canonical
portable root and all closure counts, compares the complete receipt, then
recursively live-reads the recorded portable tree/manifests/chunks. A missing
receipt, altered count/root, corrupt source closure or missing/corrupt portable
object fails closed. `fsck` performs the same verification for every receipt.
Neither path repairs state or changes refs, staging, worktree or format state.

The receipt is not a restore-capable backup, ref update, repository-format
activation, migration of history, rollback implementation or no-downgrade
marker. Those remain future migration steps. Receipts are local repository
metadata and therefore must eventually be included in the verified backup
contract rather than treated as an independent backup.

## Benchmark classification

`benchmarks/portable_manifest.py` builds the exact candidate and base in
separate targets. Both receive the identical `portable-manifest regular
payload` command. The baseline is timed where it supports the payload and is
explicitly classified as unsupported above its former 64 MiB CLI cap. Git
`hash-object -w` is a diagnostic external subset: it publishes identical bytes
but does not provide Loom's typed metadata, fixed chunk closure or necessarily
the same durability. Consequently its timing is not a pure product comparison.

`benchmarks/portable_tree.py` measures complete publication of identical native
directory arguments for candidate worker requests 1, 2, 4 and 0. The previous
Loom receives those exact argument lists; a binary without `--workers` is
classified unsupported for the version comparison. Its legacy no-worker
command is retained only as a non-equivalent diagnostic control. Git
`add --all payload` plus `write-tree` is only a diagnostic subset because its
metadata, live-verification and durability contracts differ.

`benchmarks/portable_export.py` measures complete export of identical canonical
trees to fresh destinations for worker requests 1, 2, 4 and 0. Candidate and
previous Loom receive the same `portable-export @TREE_ID DESTINATION --workers
N` argument shape and independently published trees must have the same root ID.
A previous binary without the command is classified unsupported, so no version
speed factor is reported. Git `checkout-index --all` is retained only as an
external diagnostic: it receives the same native file directory but lacks
Loom's live closure verification, typed manifests, per-file fsync and completion
receipt.

`benchmarks/portable_verify.py` measures complete verification of fresh,
identical exported workspaces for worker requests 1, 2, 4 and 0. A previous
binary without `portable-verify` is classified unsupported, so no version
factor is produced. Git `diff --no-index --quiet` compares the same two native
directory payloads only as a diagnostic subset: it does not verify Loom's
canonical closure, receipt or portable metadata contract.

`benchmarks/portable_migration_plan.py` measures fresh, byte-identical v1
revision preflights for worker requests 1, 2, 4 and 0. Candidate and previous
Loom receive identical command shapes; an older binary is classified
unsupported. Git `fsck --full --no-reflogs HEAD` is a diagnostic closure check,
not the same migration plan or portable metadata contract. The harness records
the exact started argv and requires one canonical candidate root across all
repetitions per worker request.

The same harness's `--operation publish` mode measures the complete durable,
live-verified additive publication on fresh repositories with those same
worker requests. Candidate and previous Loom again receive identical arguments;
unsupported previous binaries get no invented version ratio. Git `fsck` remains
only a closure-check diagnostic and is not presented as equivalent publication.

Its `--operation checkpoint` mode measures that same complete workflow plus the
final live verification and durable SQLite receipt transaction. Candidate and
previous Loom receive identical commands and worker requests; an older binary
without the command is classified unsupported. Git `fsck` remains a diagnostic
closure check and has no equivalent prepared-migration receipt contract.

Its `--operation verify` mode first creates and live-verifies the checkpoint
outside timing, then measures the complete read-only recomputation, receipt
comparison and live portable-closure verification. Candidate and supported
baseline receive identical directory contents, command shapes and explicit
worker requests; their resulting canonical roots must match. CLI support is
probed in separate disposable repositories so publication/checkpoint timing is
not silently converted into a warm idempotent retry. The report retains both
setup and actually timed argv. Git `fsck` remains a narrower diagnostic with no
receipt or portable-v2 contract.

The harness records p50/p95/p99 wall/CPU, peak RSS, kernel block counters and
logical/allocated object-store bytes for small, multi-chunk and above-64-MiB
payloads. Each sample uses a fresh repository. Verify setup occurs outside its
timed section; plan, publication and checkpoint are each executed only once in
the timed repository. Inputs contain deterministic distinct
high-entropy 1 MiB blocks so the large case does not collapse into repeated
chunks. OS caches are not flushed.

## Remaining migration gates

This remains prepared additive closure publication rather than an applied v1-to-v2 migration. Full Unicode
collision policy, history conversion, verified backup/restore,
activation recovery, rollback/no-downgrade state, remote transport
and three-OS golden-vector evidence remain open under issue #13. Refs continue
to use the existing atomic compare-and-swap path and do not yet reference these
manifests. Portable export is an isolated materializer, not yet a named sparse
workspace lifecycle with ownership, lease or remote authority.
