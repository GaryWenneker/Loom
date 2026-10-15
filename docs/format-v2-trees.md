# Native tree objects: version-2 envelope

**Status:** Experimental, additive **tree object** support. This does **not** migrate the existing v1 changeset history or promise complete v2 repository compatibility.

## Scope and compatibility

Existing v1 blobs and changesets retain their exact byte representations and BLAKE3 identities. Loom can optionally derive and publish immutable directory-tree objects for any existing v1 changeset using `loom tree @CHANGESET_ID`. A SQLite mapping from the changeset ID to the tree root is **derived** and may be discarded/rebuilt; the v1 changeset DAG remains authoritative.

The new tree objects use the same 15-byte header layout, but version byte **02** and object-kind byte **03**. Existing v1 objects use version **01** and kinds **01** (Blob) / **02** (Changeset). Unsupported version-kind combinations are rejected. An older Loom binary that knows only v1 may reject repositories containing v2 tree objects during integrity scans. **Downgrade compatibility is not provided.** Back up a repository before running experimental extensions.

## Canonical envelope

| Offset | Type | Meaning |
|---|---|---|
| 0–4 | 5 bytes | ASCII `LOOM` followed by NUL |
| 5 | u8 | `02` for native tree objects |
| 6 | u8 | `03` tree object kind |
| 7–14 | u64 little-endian | Payload length |
| 15 onward | bytes | Exactly the payload |

Object ID = **BLAKE3-256 over the complete canonical envelope**, independent of its path in the objectstore. Maximum payload is currently 64 MiB per tree node.

## Canonical tree payload

A directory is encoded as a flat, **one-level entry list**. Child directories point to other immutable tree objects, giving a hierarchical DAG with deduplicated unchanged subdirectories.

- `count`: u32 little-endian, 0–1,000,000 entries.
- For each entry, in strictly ascending lexicographic UTF-8 byte order:
  - `kind`: u8, `01` regular file, `02` child directory.
  - `flags`: u8; `00` no executable flag, `01` executable file. For directories only `00` is valid.
  - `name_length`: u32 little-endian, 1–1,000,000.
  - `name`: exact UTF-8 bytes of **one** safe path component, no slash, backslash, NUL, traversal or reserved repository metadata names.
  - `object_id`: 32 raw BLAKE3 object-ID bytes. Files reference v1 blobs, directories reference v2 tree objects.
- No trailing bytes.

Decoders reject duplicates, unsorted names, invalid UTF-8, oversized counts/names, unsupported kinds, invalid flags, truncated entries and incorrect object envelope versions. Materialization validates object types and limits directory nesting to 256 components.

### Example: empty tree

The canonical empty tree is a 19-byte envelope:

```text
4c 4f 4f 4d 00 02 03 04 00 00 00 00 00 00 00 00 00 00 00
|  LOOM+NUL  |v2|tree|     u64 payload=4       |u32 count=0|
```

Direct object reads are available using `loom tree ls @TREE_ID [DIRECTORY]` and `loom tree cat @TREE_ID PATH`. These follow only the selected immutable directory path and verify every opened object's hash; they do not need a checkout or SQLite branch state.

The serialized content of a tree is independent of the insertion order used to construct it. Equal directories have equal object IDs. If a file changes in one directory, unaffected sibling trees keep their original object IDs.

## What this implementation actually preserves

The current v1 worktree and changesets only track regular-file bytes and UTF-8 names. Therefore converting existing commits marks all regular files as **non-executable**; it cannot recover historical executable bits, symlinks, ownership, permissions or extended attributes that were never recorded. An executable-bit field in the tree schema is reserved for a future fully metadata-aware history format and does **not** imply that mode preservation currently works.

The converter refuses unsafe paths, file-vs-directory collisions, excessive depth and objects exceeding the current per-node limit.

## Integrity and recovery

`loom tree @REVISION` resolves the closest materialized first-parent ancestor root, or starts with the canonical empty tree when no checkpoint exists. It walks subsequent changesets and applies their file deltas using batched copy-on-write mutations: only directories along affected paths are re-encoded, and unchanged subtrees retain their IDs. It then stores an optional SQLite revision-to-root mapping. Both legacy commit-all and explicitly staged commits persist the new immutable tree nodes before atomically publishing the revision root alongside the branch compare-and-swap. Staged commits additionally update the branch path index and clear staged entries in the same transaction. Missing parent roots are lazily reconstructed from immutable history once. Merge transitions still publish a root on demand. A first uncached lookup must still examine changeset metadata along the ancestor chain. On process interruption, previously published immutable nodes may remain unreferenced, but the original changeset history is not modified.

`loom fsck` inspects all stored tree nodes, verifies their child object types and hashes, and cross-checks each derived revision-to-root mapping against the authoritative changeset file state. A corrupt mapping is rejected. Absence of a mapping does not lose source history: a later `loom tree` can regenerate the same root ID.

**Remaining release blockers:** complete v1→v2 migration/rollback, portable path collision rules, historic file modes/symlinks, chunked large-file manifests, automatic durable tree-root publication for legacy commit-all and merge transitions, crash/power-loss safety and distributed object/ref protocols. See [release gates](release-gates.md) and [roadmap](roadmap.md).
