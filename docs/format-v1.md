# Loom native object format v1

**Status: experimental; not yet a stable compatibility contract.**

Object ID: lowercase hex encoding of BLAKE3-256 over the entire canonical envelope.

Envelope layout (offsets in bytes):
- 0..5: ASCII `LOOM\\0` (five bytes including NUL).
- 5: version `01`.
- 6: type: `01` Blob, `02` Changeset.
- 7..15: unsigned little-endian u64 payload length.
- 15..: exact payload bytes, no trailing bytes.

v1 object size cap: 64 MiB payload.

Blob payload: exact file bytes.

Changeset payload:
1. unsigned u8 parent count (0..2).
2. Each parent object ID as exactly 32 bytes. First parent determines the delta base.
3. Signed i64 little-endian timestamp (seconds since Unix epoch).
4. Author: u32 LE byte length + UTF-8 bytes.
5. Message: u32 LE byte length + UTF-8 bytes.
6. Change count: u32 LE (max 1,000,000).
7. Changes sorted lexicographically by their UTF-8 path bytes, each consisting of: u8 operation 1 add/2 modify/3 delete; u32 LE UTF-8 path byte length + bytes; 32-byte blob object ID unless delete.

Duplicate paths, non-normalized paths and unsupported object types are invalid. Paths are separated by `/`, cannot contain traversal, backslashes or reserved `.loom` and `.git` segments.

Files are regular files in v1: symlinks, executable bits, non-UTF-8 filenames, giant files and file/directory conflict transitions are **not yet** supported. Do not silently claim preservation of unsupported metadata.

A production-grade v2 will require a formal migration strategy, richer file types, chunking and proof of portable deterministic test vectors.
