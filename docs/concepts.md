# Native concepts

Loom uses immutable BLAKE3-256 addressed objects in its own versioned binary envelope. The experimental v1 format supports blobs (regular file bytes) and changesets (file deltas with zero, one, or two parent IDs). The first parent determines delta replay; the second preserves merge ancestry. See [format v1](format-v1.md).

A **branch** is a mutable SQLite reference to a changeset. **HEAD** names the current branch. Updates use expected-old-value compare-and-swap.

The **staging index** records the exact content IDs selected by `loom add`; later filesystem edits are separate. `loom review` shows both prepared and remaining changes. Explicit staging is opt-in in the current alpha, not a finalized workflow contract.

**Worktree recovery** uses a SQLite intent record and staged file replacement for selected branch switches and merges. An interruption can be replayed under a local lock; unexpected modifications block recovery. It is not a cross-filesystem transaction or proven sudden-power-loss guarantee.

The planned architecture includes native tree objects, chunk manifests, indexed ancestry and path history, isolated workspaces, sparse reading, remote sync and a separate Git adapter. These are not present in v1.

See [architecture](architecture.md), [roadmap](roadmap.md) and [release gates](release-gates.md).