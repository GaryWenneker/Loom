# Loom architecture

Loom is a standalone version control system with its own object storage engine and history model.

## Invariants

1. Immutable objects are addressed by BLAKE3-256 of their canonical native envelope.
2. Refs point only to already durable changesets; updates use optimistic compare-and-swap.
3. A failed CAS does not destroy immutable objects.
4. Replay of a changeset applies its delta to its **first parent** only; other parents capture merge ancestry.
5. Corrupt, missing, or ill-formed objects must surface as errors.
6. Workspaces are untrusted: traversal paths and symlinks must never escape the repository.
7. Derived indexes may be discarded and rebuilt.
8. Server mode and Git adapters cannot change canonical object IDs.

## Current local data path

Legacy commit-all reads and hashes regular working files, persists changed blob envelopes through bounded publication workers, writes a first-parent changeset and a copy-on-write native directory-tree root, then atomically publishes the revision/root mapping and branch ref in SQLite. Explicit staging commits immutable selected blobs without rescanning unrelated files and updates staging/path indexes in the same ref transaction.

The full-content scan implementation in PR #86 adds count/byte windows and capped read/hash workers to commit-all and status. It still reads every working file; it is not incremental change discovery. Revision-pinned query sessions already support direct tree/path reads, verified caches and an optional read pack without checkout. Authoritative v1 changesets remain reconstructable without derived indexes.

The initial engine uses loose objects under `.loom/objects/xx/yyyy...` and local SQLite metadata. Disk writes use temporary files and rename before the reference is committed. On Unix, object directory syncs are included; other OS durability needs targeted audits and fault tests.

## Future layers

- Metadata-complete native histories, chunk manifests, pack compaction and scalable ancestry indexes.
- Multi-parent merge/rebase/undo engine and sparse/virtual workspaces.
- Version-negotiated streaming gRPC service and consistent remote refstore.
- Cache hierarchy, distributed object storage and resumable object transfer.
- Standalone Git import/export/bridge crate, preserving compatibility metadata.

The full target design is specified in [implementation contracts](implementation-contracts.md). Release quality and no-data-loss evidence are defined in [release gates](release-gates.md). Progress and dependencies are tracked in the [roadmap](roadmap.md).

The engine has optional v2 native directory trees alongside its v1 blob/changeset history, and an explicit bridge to an installed Git executable. It does **not** yet provide metadata-complete native histories, Git history import/export or a native remote server. These remain implementation work rather than released capabilities.
