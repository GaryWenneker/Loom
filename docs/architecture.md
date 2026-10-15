# Loom architecture

Loom is a **standalone** version control system. It does not depend on Git or AX.

## Invariants

1. Immutable objects are addressed by BLAKE3-256 of their canonical native envelope.
2. Refs point only to already durable changesets; updates use optimistic compare-and-swap.
3. A failed CAS does not destroy immutable objects.
4. Replay of a changeset applies its delta to its **first parent** only; other parents capture merge ancestry.
5. Corrupt, missing, or ill-formed objects must surface as errors.
6. Workspaces are untrusted: traversal paths and symlinks must never escape the repository.
7. Derived indexes may be discarded and rebuilt.
8. Server mode and Git adapters cannot change canonical object IDs.

## MVP data path

`working tree -> scanner -> blob envelopes -> changeset envelope -> durable objectstore -> SQLite CAS ref`

The initial engine uses loose objects under `.loom/objects/xx/yyyy...` and local SQLite metadata. Disk writes use temporary files and rename before the reference is committed. On Unix, object directory syncs are included; other OS durability needs targeted audits and fault tests.

## Future layers

- Native trees, chunk manifests, packfiles, incremental history indexes.
- Multi-parent merge/rebase/undo engine and sparse/virtual workspaces.
- Version-negotiated streaming gRPC service and consistent remote refstore.
- Cache hierarchy, distributed object storage and resumable object transfer.
- Standalone Git import/export/bridge crate, preserving compatibility metadata.

The full target design is specified in [implementation contracts](implementation-contracts.md). Release quality and no-data-loss evidence are defined in [release gates](release-gates.md). Progress and dependencies are tracked in the [roadmap](roadmap.md).

The existing v1 engine does **not** yet have native trees, metadata-complete histories, a Git adapter or a remote server. A documented architectural intention is **not** implemented functionality.
