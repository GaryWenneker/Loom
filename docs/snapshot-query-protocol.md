# Revision-pinned query sessions

`loom query --batch @CHANGESET_ID [--cache-bytes N]` serves repeated immutable
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
listing traverses at most 1,000,000 entries. Pagination currently restarts tree
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

The per-session FIFO cache retains verified decoded trees and blob payloads.
Default budget is 64 MiB, configurable from zero (disabled) through 256 MiB.
Accounting includes content capacity and estimated per-entry bookkeeping; it is
not a process RSS limit and excludes Arc values retained by API consumers.
Stats expose physical tree/blob reads, cache hits, entries and accounted bytes.

SQLite providers retain one mutex-protected **read-only** connection per session.
Other providers can opt into the separate `ObjectReadSession` capability, with a
live-read fallback. The core facade validates envelope/hash even for custom
readers. These handles cannot acknowledge writes or verify live durability.
Already verified content may remain readable after external corruption/deletion;
a retained SQLite handle may also outlive a storage pathname. New sessions,
publication, recovery and `fsck` continue to use live provider checks. A query
validates requested objects, not every historical object or every replica.
Strict normal repository discovery still rejects incomplete external history.

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
