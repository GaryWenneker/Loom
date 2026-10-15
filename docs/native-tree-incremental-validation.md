# Canonical native tree updates

Loom's native tree writer prunes empty non-root directories. Tree flattening rejects an empty child directory as noncanonical. Incremental updates now enforce the same rule when opening a directory along an edited path: an existing empty child tree is rejected instead of being silently normalized by adding a new file.

This guard protects canonical input validation without scanning untouched sibling subtrees. It is **not** a full transitive-closure audit of the previous root; use `flatten_snapshot` for complete traversal and blob checks. Existing native tree identities and serialization are unchanged. The v1-to-v2 migration, cross-platform metadata semantics and conformance suite remain outstanding under issue #13.
