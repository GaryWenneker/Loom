# Disposable read-pack format 1

This is an optional local acceleration format, not a canonical Loom object or
an authoritative object store. Delete/rebuild it without migrating repositories.
See [usage and resource/integrity contract](snapshot-query-protocol.md#optional-immutable-read-packs).

All integers are unsigned little-endian. File layout:

| Region | Encoding |
| --- | --- |
| Header | Eight literal bytes `LOOMRDP1` |
| Data | Unmodified canonical envelopes, in increasing 32-byte object-ID order |
| Index | One 48-byte record per object: ID (32), absolute offset (u64), envelope length (u64) |
| Footer | `LOOMIDX1` (8), index absolute offset (u64), count (u64), BLAKE3-256 of the complete index (32) |

The 56-byte footer ends exactly at EOF. Count is 1..=1,000,000; whole-file size
is at most 256 MiB. Index arithmetic is checked before allocation. IDs are strictly
increasing and unique. The first object begins at offset 8, records describe
contiguous non-overlapping data, and the final envelope ends at the index start.
Each envelope length is 15..=64 MiB+15. There are no trailing or padding bytes.
The index hash detects corruption; it is not a trusted signature. Each requested
object must separately match its ID and canonical envelope. The snapshot parser
then validates tree syntax and expected file/directory types as usual.

Building walks one immutable root, validates referenced types, bounds nesting
and traversal, and writes one object at a time. It syncs a new temporary file,
hard-links it to a previously absent destination, removes the temporary alias,
and on Unix syncs the containing directory. No old pack or live object is
replaced/deleted. Interrupted/unpublished temporary files are never queried.
The same atomic-create requirement as loose objects applies; unsupported hard
links cause an error. This optional cache has no data-durability authority.

The storage hot path also encodes object IDs into one fixed 64-byte ASCII buffer
instead of invoking the formatter for each byte. Text stays lowercase, and
parsing still accepts either hex case. Parsing now explicitly requires hex
digits; previous accidental numeric `+f`-style aliases are rejected as specified
by the existing 64-hex-digit contract. Canonical object bytes and IDs do not change.
