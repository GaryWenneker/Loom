# ADR-002: Independent Git interoperability

**Status:** Proposed

Git translation must be implemented in a separate `loom-git` crate. The native object envelope and IDs cannot contain or depend on Git object bytes.

Future adapter milestones:
1. Explicit import of Git commit DAG, blobs, refs and tags.
2. Stable persistent Git OID to Loom OID mapping and compatibility reporting.
3. Export of native history to a valid Git repository.
4. Incremental safe synchronization, including branch divergence handling.
5. Optional Git protocol gateway.

Renames, symlinks, executable bit, signed commits/tags, submodules, LFS and unusual filenames must have tests. Unsupported constructs must produce warnings/errors, not silent corruption.

Git export into GitHub remains unavailable until integration tests confirm semantic equivalence.
