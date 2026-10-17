# Portable v2 blob manifests

Loom can publish an additive `PortableBlobManifest` without changing v1
history or repository format selection:

```sh
loom portable-manifest regular path/to/file
loom portable-manifest executable path/to/tool
loom portable-manifest symlink path/to/link
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

The current CLI reads one regular file into memory and therefore rejects input
larger than 64 MiB. The core manifest schema is chunked, but bounded streaming
publication and handle-relative race protection are still migration gates.

## Benchmark classification

`benchmarks/portable_manifest.py` builds the exact candidate and base in
separate targets. The previous Loom binary receives the identical
`portable-manifest regular payload` arguments; because it lacks the command it
is recorded as unsupported and receives no speed factor. Git `hash-object -w`
is a diagnostic external subset: it publishes identical bytes but does not
provide Loom's typed metadata, fixed chunk closure or necessarily the same
durability. Consequently its timing is not a pure product comparison.

The harness records p50/p95/p99 wall/CPU, peak RSS, kernel block counters and
logical/allocated object-store bytes for 1 KiB and 8 MiB payloads. Each sample
uses a fresh repository. Verification and idempotent retry occur outside the
timed section. OS caches are not flushed.

## Remaining migration gates

This is a contract primitive, not a v1-to-v2 migration. Native tree symlink
entries, portable path collision rules, history conversion, backup/restore,
crash recovery, rollback/no-downgrade state, streaming input, remote transport
and three-OS golden-vector evidence remain open under issue #13. Refs continue
to use the existing atomic compare-and-swap path and do not yet reference these
manifests.
