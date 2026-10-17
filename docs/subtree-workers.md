# Bounded native subtree construction

Commits apply their edit trie to the previous immutable native-tree root. Loom
follows shared prefixes such as `src/modules/` until at least two independent
changed directory entries are available. Cheap file-entry edits stay serial;
directory tasks are grouped separately so they cannot be hidden in one chunk of
otherwise cheap work. This also covers two/eight-directory selections below a
shared parent, which the previous root-only, 32-entry threshold left serial.

`--scan-workers N` supplies the commit worker request as well as the selected
worktree read/hash request:

- `0` selects up to four workers;
- explicit requests are capped at eight and at available CPU parallelism;
- native subtree writes are additionally capped by the storage provider's safe
  `max_concurrent_writes` hint, shared with the buffered object publisher;
- fewer than two changed directories continue serially with their worker budget
  available further down the shared path.

Once a node forks, work below each selected directory stays serial. Thus a
deep/wide tree cannot recursively multiply pools. A replicated provider's outer
one-write hint also prevents subtree workers from multiplying its internal
replica workers. These are per-call bounds, not a global budget across unrelated
repository operations.

Scanning, immutable-object publication and subtree construction remain separate,
joined phases. Each worker receives one removed entry and an immutable edit
subtree. All workers join before parent encoding or failure return. BTreeMap
ordering and canonical encoding preserve object IDs regardless of scheduling.
Failed builds can leave unreachable immutable objects; the changeset, root
mapping and branch ref are not acknowledged before the normal CAS transaction.

Regression tests exercise a common two-level prefix, mixed cheap file edits and
independent directory updates with further nested branches. An observed provider
requires actual overlap, rejects writes beyond its one/two-worker limit, and
checks identical serial/parallel roots and all paths. A native-tree write failure
also preserves the branch and complete staged snapshot, drains workers, and
permits a lossless retry. Existing canonical/path/metadata tests remain active.

`benchmarks/pathsets.py --changed-paths 64 --scan-workers 1` and the same request
with `8` compare complete checked add-plus-commit histories with the exact
pre-change Loom baseline and Git. Setup, warm-cache conditions, split command
timings, canonical roots, full payload/fsck checks and raw resource limitations
remain explicit. Exposing or enabling a pool is not itself a speedup claim.
