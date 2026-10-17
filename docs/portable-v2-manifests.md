# Portable v2 blob manifests

Loom can publish an additive `PortableBlobManifest` without changing v1
history or repository format selection:

```sh
loom portable-manifest regular path/to/file
loom portable-manifest executable path/to/tool
loom portable-manifest symlink path/to/link
loom portable-tree path/to/directory --workers 0
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

The harness records p50/p95/p99 wall/CPU, peak RSS, kernel block counters and
logical/allocated object-store bytes for small, multi-chunk and above-64-MiB
payloads. Each sample uses a fresh repository. Verification and idempotent
retry occur outside the timed section. Inputs contain deterministic distinct
high-entropy 1 MiB blocks so the large case does not collapse into repeated
chunks. OS caches are not flushed.

## Remaining migration gates

This is a contract primitive, not a v1-to-v2 migration. Full Unicode collision
policy, history conversion, backup/restore,
crash recovery, rollback/no-downgrade state, remote transport
and three-OS golden-vector evidence remain open under issue #13. Refs continue
to use the existing atomic compare-and-swap path and do not yet reference these
manifests.
