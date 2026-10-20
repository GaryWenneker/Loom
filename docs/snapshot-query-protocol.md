# Revision-pinned query sessions

`loom query --batch @CHANGESET_ID [--cache-bytes N] [--read-pack FILE]` serves repeated immutable
reads over stdin/stdout. It opens existing metadata read-only, takes no workspace
lock, does not recover a pending checkout/merge, and cannot update refs or staging.
Independent agents can read while another process owns the workspace lock.
It does not create an independent writable workspace or provide remote isolation.

The revision must already have a materialized native tree. Current commits do;
for older revisions run `loom tree @CHANGESET_ID` first. That explicit preparation
uses the normal repository/recovery path. Read-only queries never migrate schemas
or construct missing derived indexes. External providers retain their normal
`LOOM_OBJECT_DIR` / `LOOM_OBJECT_DB` binding; existing opens never create storage.

## Protocol 1

Each request is one UTF-8 JSON line, at most 64 KiB including its newline.
Unknown fields and operations are rejected. Request IDs are unsigned 64-bit
integers, echoed in replies. Example input:

```json
{"id":1,"op":"cat","path":"src/main.rs"}
{"id":2,"op":"files","prefix":"src","after":null,"limit":100}
{"id":3,"op":"stats"}
```

Each response starts with a JSON line containing `protocol:1`, `id`, `ok`, and
`kind`. Successful `cat` uses `kind:"blob"` and `size:N`; exactly N raw bytes and
one LF follow the header. Binary content is neither JSON escaped nor base64
encoded. Missing files/directories return `kind:"missing"` with no payload.
`files`, `stats`, and errors are JSON-only. Every reply is flushed immediately.
Clients must consume the declared binary length before reading another header.

`files` returns sorted full relative `paths` and an exclusive `next_cursor`
(last emitted path when more remain). Prefixes match complete path components,
including an exact file. Omitted limit is 10,000; accepted limits are 1..=100,000.
A page contains at most 8 MiB of path bytes. Nesting is limited to 256 and each
listing traverses at most 1,000,000 entries. The queued traversal frontier has
a separate 64 MiB accounted limit and fails explicitly if exceeded; use a
narrower prefix for such trees. Pagination currently restarts tree
traversal, so many tiny pages are not an efficient full-repository export.

Malformed requests receive an error with `id:null`; valid requests that fail
receive their original ID. Both allow subsequent requests. Oversized input or
I/O failure terminates the session. Safe relative paths use the existing Loom
path policy. There is no socket, daemon, authentication or branch-name tracking.

## Storage and integrity

Core callers use `Repository::snapshot` on an open repository, or
`Repository::query_snapshot` for the isolated read-only opener. `Snapshot` pins
the immutable tree root; later ref changes do not change answers. The disposable
revision-to-root index is trusted for normal queries, as by existing tree reads.
`fsck` independently reconstructs canonical history to check those mappings.
This feature does not change object IDs or the on-disk schema.

The per-session FIFO cache retains verified immutable tree bytes, compact
entry-offset indexes, blob payloads and exact file-path answers (including
missing/directory answers). Names borrow the verified tree buffer;
ordinary owned trees and read views share one canonical validator.
Default budget is 64 MiB, configurable from zero (disabled) through 256 MiB.
Accounting includes content capacity and estimated per-entry bookkeeping; it is
not a process RSS limit and excludes Arc values retained by API consumers.
Snapshots cloned from one `ObjectStore` additionally share a 256 MiB retained
cache ceiling. Admission never waits for another long-lived snapshot: the local
FIFO is evicted where possible, then an entry is skipped when the shared ceiling
is still occupied. The verified query result is returned either way. Dropping a
snapshot releases all of its retained reservation. Separately opened stores and
processes, read-pack indexes, caller-retained Arcs, temporary reads and OS cache
remain outside this ceiling, so it is still not a total RSS limit.

Stats expose physical tree/blob reads, cache hits, entries, accounted bytes and
`cache_admission_rejections` for entries skipped under shared pressure.
Sharing ancestors can reduce cache lookups/hits without increasing physical I/O.
`path_cache_hits` and `path_cache_entries` identify exact-path reuse. Total
`cache_entries` and `cache_hits` include both object and path entries.

Path answers are private to this immutable root. Cache misses validate path size,
syntax and nesting before I/O; exact hits reuse only previously validated keys
and verified answers. Ref/worktree changes cannot make an old root's missing
path appear. Scalar and grouped reads share these answers. A group checks its
returned-byte limit even on cached duplicates, and inserts new path answers only
after the whole group succeeds. All cache mutations remain mutex protected.

Object and path entries compete in the same FIFO and the same byte budget. Path
accounting includes both owned path strings and bookkeeping, and conservatively
counts the payload per path even when Arc content is shared with another entry.
Zero budget disables both. Retained answers are not live-existence authority;
fresh views without a selected pack and all publication/fsck paths still use
live storage. A hot working set can avoid further tree walks. It does not remove
JSON parsing, response framing, or first-group storage costs. CPU/cgroup worker
discovery is deferred until enough uncached blobs need parallel I/O. Serial
providers skip CPU discovery.

SQLite providers retain one mutex-protected **read-only** connection per session.
Other providers can opt into the separate `ObjectReadSession` capability, with a
live-read fallback. The core facade validates envelope/hash even for custom
readers. These handles cannot acknowledge writes or verify live durability.
Already verified content may remain readable after external corruption/deletion;
a retained SQLite handle may also outlive a storage pathname. Sessions without
an explicit read pack, publication, recovery and `fsck` use live provider checks. A query
validates requested objects, not every historical object or every replica.
Strict normal repository discovery still rejects incomplete external history.

## Optional immutable read packs

```sh
loom read-pack @CHANGESET_ID .loom/reads.pack
loom query --batch @CHANGESET_ID --read-pack .loom/reads.pack
```

The preparation command captures that revision's native tree/blob closure in one
uncompressed file. It checks live objects and their expected types, deduplicates
IDs, then publishes a complete file with atomic create-if-absent. An existing
destination is never overwritten. Use a new filename when rebuilding. A failed
build removes its temporary file; process termination can leave an unused
`.loom-read-pack-*.tmp` alongside the destination. Queries never discover those
files. Preparation uses normal repository discovery/recovery and can materialize
a missing revision tree. Run it explicitly, outside latency-sensitive reads.

Only `--read-pack FILE` opts a query into the pack. The read-only opener still
checks the live revision changeset and metadata/provider binding. Pack misses
fall back to the normal read session, so a pack can accelerate later revisions
too. A missing or malformed explicitly selected pack fails the opener; rebuild
it from live storage or omit the flag. Corrupt present objects fail their request
instead of silently falling back. Every fetched envelope is hash/type/length
checked; the index checksum alone never authenticates content.

Packs are disposable acceleration files. They are **not backups**, do not
replace loose/SQLite objects, and cannot satisfy fsck, recovery, ref publication,
replica closure or live-existence checks. Canonical object bytes/IDs and database
schemas stay unchanged; old binaries can still use the repository. A retained
pack handle may outlive its pathname, just like other retained immutable readers.
No remote authorization boundary is introduced.

Each file is capped at 256 MiB and 1,000,000 indexed objects. Its full sorted
index is loaded and checked at session startup: up to 48,000,000 bytes retained
in one buffer, without a second decoded-index allocation. `stats` exposes `read_pack_objects`
and `read_pack_index_bytes` separately from the snapshot cache budget. Pack data
is read on demand, not all retained in memory. An object envelope buffer, copied
payload, owned tree parsing during preparation and OS page cache are additional;
these caps are not a process RSS limit. The pack session requests one object
worker. Unix uses positioned reads; Windows serializes seek/read pairs.

The disposable format is documented in [read-pack format](read-pack-format.md).
Unix rejects leaf symlinks, non-regular files and persistent extra hard links.
Windows checks for pre-existing symlinks. Parent-path substitution races retain
the existing filesystem contract limits. Replacing or editing a pack concurrently
may cause a read error; content verification still prevents incorrect bytes.

## Compatibility and rollback

Existing commands and provider implementations remain compatible; the reader
capability has a default fallback. Scoped historical file listing now traverses
native trees instead of materializing a full SQLite path index; scalar cache
misses use an available native root before falling back to history replay.
The canonical replay used by `fsck` remains independent of these shortcuts.
Rollback removes the new command/API; no data migration is needed. Existing
symlink race limits, 64 MiB object limits and experimental release status apply.

See [benchmark reproduction](../benchmarks/README.md) for operation-specific
comparisons, raw samples, and the limits of the 100× performance target.


## Native grouped file reads

```json
{"id":7,"op":"cat_many","paths":["src/main.rs","missing","src/lib.rs"],"max_bytes":16777216}
```

Core callers use `Snapshot::read_files(paths, max_bytes)`. Results preserve input
order, including duplicates. Missing files and directory paths occupy null slots.
The maximum is 4,096 paths, also subject to the 64 KiB JSON request limit. Default
returned-content budget is 16 MiB, configurable from zero through 64 MiB. Repeated
paths count their bytes repeatedly toward this budget. Zero allows only empty
files/misses. Unsafe paths, corruption and byte-budget failures fail the entire
group before any successful header/payload is emitted; later requests still work.

Success has `kind:"blobs"`, `entries:[{"size":N},null,...]` and aggregate `size`.
After the JSON header LF come exactly `size` raw bytes, concatenated in slot order,
then one LF. There are no per-file separators; null slots consume no bytes.
An empty file is `{"size":0}`, distinct from null. A failed group is a JSON-only
error. This extends protocol 1; existing `cat`/`files`/`stats` frames are unchanged.
Output is buffered within each reply and always flushed before awaiting another
request, including interactive clients that keep stdin open.

With a nonzero cache budget, grouped reads resolve each requested parent once
and use retained verified ancestors. With zero cache, they traverse requested
components by depth instead: each directory object is loaded once per depth,
sharing ancestors and identical directory DAGs without repeated storage reads.
Frontier records contain input-slot indices and byte offsets into the already
validated input strings; the 4,096-path limit bounds each frontier. Directory
work is sorted by object ID for filesystem locality and runs serially. Each tree
is released after routing its queries, so only one active tree is needed outside
the cache, without retaining a stack or a layer of large tree buffers.

Deduplicated blob work is likewise sorted by object ID. Groups of at least 32
use scoped workers, at most eight and no more than the provider hint/available
CPU count. Filesystem read sessions currently request two workers; SQLite and
other serial sessions retain the default of one. Blob cache entries are
published after all blob workers succeed. Path answers are published only after
the entire request succeeds. Every worker is joined on success, error or panic
before the call returns.

`max_bytes` limits returned content, not peak RSS. The separate snapshot cache,
indexes, reply metadata and temporary object-read buffers are additional. Each
active object worker can temporarily hold an envelope and a copied payload under
the existing 64 MiB object limit. This API is not a streaming large-file reader.

Unix filesystem sessions pin a base-directory handle and at most 64 cached
fanout-directory handles, with additional temporary handles for active reads.
Object opens are relative to these handles and reject leaf/fanout symlinks;
nonblocking opens allow rejection of FIFOs before they can block. Eviction closes
unused handles. A pinned directory may outlive its old pathname, consistent with
the immutable read-session contract. Initial ancestor-path race limits remain;
this does not change the live provider used for publication or fsck. Windows uses
its existing verified filesystem reads. No storage files or formats are changed.
