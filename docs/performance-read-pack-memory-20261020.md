# Shared read-pack index memory, 20 October 2026

## Functional change

An explicitly opened read pack retains a sorted 48-byte record per object.
Previously the 256 MiB pack-file and one-million-object limits bounded each
session independently, so cloned snapshots could multiply those indexes outside
the shared snapshot-cache ceiling. The opener now derives the index size from
the validated footer and reserves those bytes before allocating or reading the
index. Snapshot caches and pack indexes cloned from one `ObjectStore` share the
same 256 MiB process-local retained-memory limit.

Cache admission remains nonblocking and may skip an entry. A requested pack is
explicit configuration and cannot be skipped silently: opening fails with a
resource error while the shared limit is occupied. The RAII reservation is
released when validation fails or the snapshot drops, so retry after pressure
subsides works. Canonical envelopes, object IDs, live fallback reads, storage
durability, replica verification, closure checks and ref CAS are unchanged.

This is not a total RSS cap. Independent stores/processes, returned `Arc`s,
temporary object buffers, allocator overhead and the OS page cache remain
outside it.

## Correctness and performance evidence

The regression constructs a real pack, sets the shared limit to exactly one
index, keeps one packed snapshot live, verifies a second opener fails, drops the
first snapshot and verifies a retry reads the expected bytes. The normal read
pack validation and cross-platform suites remain applicable.

The PR benchmark builds exact candidate and previous-main binaries in separate
Cargo targets. Each binary builds its own pack from the same revision; timed
processes use identical native `cat_many` requests, zero snapshot cache and the
same 64 directory arguments. Candidate, previous Loom and packed Git results
are retained without treating unequal contracts as equivalent. Pack build time
is outside query timing and reported separately.

The benchmark records wall-clock p50/p95/p99, binary/harness/tree hashes,
repository footprint and exact arguments. Its current harness does not record
CPU time, peak RSS, block I/O, fsyncs, cache hits, network, cold-cache behavior,
contention or recovery duration; those measurements remain explicitly missing.
No speedup or 100x claim follows from the memory bound alone.
