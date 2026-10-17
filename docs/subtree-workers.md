# Bounded native subtree construction

Commits apply their change trie to the previous immutable native-tree root. When
at least 32 distinct root entries change, Loom may build those independent
subtrees concurrently. Work below each selected root entry stays serial, so a
wide/deep changeset cannot recursively multiply worker pools.

`--scan-workers N` supplies the commit worker request as well as the selected
worktree read/hash request:

- `0` selects up to four workers;
- explicit requests are capped at eight and at available CPU parallelism;
- fewer than 32 independent root entries run serially.

Scanning, immutable-object publication and subtree construction are separate,
joined phases. They never add their worker counts concurrently. Object
publication keeps its backend-specific bounded queue.

Each worker receives one removed root entry plus its immutable edit subtree.
Workers may read and publish only content-addressed objects. Results are joined
in the original `BTreeMap` order and the parent is encoded canonically, so worker
count and completion order cannot affect object IDs. Every worker is joined
before an error returns. A failed build can leave unreachable immutable objects,
but the changeset, tree-root mapping and branch ref are not published; the
existing compare-and-swap ref update remains the commit point.

The regression test applies 64 independent subtree updates with one and eight
requested workers and requires identical roots and payload lookup results.
`benchmarks/pathsets.py --changed-paths 64` measures the same complete
add-plus-commit histories for one/eight-worker candidate runs, the exact
pre-change Loom baseline and Git.
