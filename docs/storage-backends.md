# Pluggable storage architecture

## Current implementation

Loom's repository engine routes canonical objects through `ObjectBackend`. The default remains filesystem fanout directories under `.loom/objects`. Callers can instead supply a durable provider through `Repository::init_with_object_backend` and `Repository::discover_with_object_backend`; native trees, blobs, history, staging and transitions all use that provider. SQLite WAL continues to own local refs, staging, indexes and the workspace journal in their existing transactions.

Implemented providers are filesystem and SQLite objects, ephemeral memory objects/refs, independent SQLite ref CAS, and verified all-ack immutable object replication. The independent ref-provider API does not replace the repository's SQLite journal transaction. Cloud locations are validated configuration contracts; AWS, Azure and GCP network drivers remain unimplemented.

## Use a separate object directory

External object files must live outside the workspace, or a legacy full-worktree snapshot could track its own store. CLI initialization rejects storage inside the worktree, including existing symlink aliases, before opening the provider. A new storage location requires an existing parent directory. Embedding callers must also keep their provider files outside the worktree.

```sh
# Use an absolute path, also when invoking Loom from a subdirectory.
export LOOM_OBJECT_DIR=/absolute/path/loom-objects
loom init /absolute/path/workspace
cd /absolute/path/workspace
loom add file.txt
loom commit -m "Store objects separately"
loom fsck
```

Alternatively initialize with `loom init [path] --object-dir /absolute/path/loom-objects`, then set `LOOM_OBJECT_DIR` for subsequent commands. PowerShell uses `$env:LOOM_OBJECT_DIR = 'C:\absolute\loom-objects'`. Relative paths resolve against the process working directory. Object directories may be shared across workspaces; refs/staging remain separate. This is not a shared-ref or multi-workspace server feature.

Repositories initialized with an external provider persist only the binding `external-v1`, never provider credentials or paths. Reopening without an explicit provider fails clearly. An existing local repository cannot be switched by setting the environment variable: verified object migration is separate work. An empty or incomplete replacement provider is rejected before workspace recovery.

## SQLite object storage

```sh
export LOOM_OBJECT_DB=/absolute/path/objects.sqlite
loom init /absolute/path/workspace
cd /absolute/path/workspace
loom add file.txt
loom commit -m "SQLite objects"
loom fsck
```

Alternatively use `loom init [path] --object-db FILE`, then set `LOOM_OBJECT_DB` for later commands. The database parent directory must exist. An explicit init storage option overrides environment selection; passing both init options or setting both provider environment variables is an error. Ordinary repositories keep their default filesystem storage.

The object database uses WAL and `synchronous=FULL`, immutable conditional insertion in an immediate transaction, on-read hash/envelope verification and complete ID enumeration for `fsck`. A bounded publisher window is inserted and verified in one atomic transaction; failures roll the whole new window back, and the core performs live readback again after commit. One operation-scoped write connection can serve successive windows, but each keeps its own durable transaction and independent fresh readback. The session checks database-file identity around every window and is dropped with the publisher. Reopening a provider never recreates a database that disappeared after initialization. Relative database paths are resolved to an absolute path when the provider opens. In-memory SQLite and database symlinks are rejected. This provider does not claim protection from all concurrent filesystem substitution attacks or hardware-specific power loss. See [Atomic SQLite object batches](sqlite-batch-publication.md) and the [pinned SQLite runtime contract](sqlite-runtime-version.md).

SQLite can be composed with filesystem providers in `ReplicatedObjectBackend`; all object IDs and canonical bytes stay identical across both. Tests reconstruct one repository from either complete copy. SQLite refs and the repository journal remain independent roles. Live databases may have WAL/SHM sidecars; copying only the main database file while active is not a verified backup procedure.

## Provider API and publication checks

`FilesystemObjectBackend::open(path)` and `sqlite_object_storage::SqliteObjectBackend::open(path)` expose the filesystem and SQLite implementations for embedding and composition. Supply an `Arc<dyn ObjectBackend>` to initialize/reopen with another implementation, including `ReplicatedObjectBackend`. Providers must advertise durable writes and conditional creation; volatile memory is rejected before initialization modifies the workspace.

The repository verifies every returned object hash/envelope and reads back each publication. Before publishing an external-provider branch ref or replaying a workspace transition, it verifies the full typed history/blob/tree closure, including historical blobs no longer in the current snapshot. Replicated providers must report each object present on every replica. Traversal is iterative and capped at one million typed objects; exceeding the cap fails before publication. This conservative check scans history and is not an agent-scale performance claim. Providers remain responsible for the accuracy of their durability/consistency declarations and retention; integrity checks cannot prevent out-of-band deletion after verification.

Full `fsck` requires authoritative enumeration: advertise `list_objects=true` and implement `ObjectBackend::list_ids`. Providers without listing (including the current replicated provider) explicitly reject full-store `fsck`; normal commits and closure validation do not depend on listing. Existing filesystem race/power-loss limitations remain in force.

## Bounded blob publication

Legacy commit-all prepares changed canonical blobs in windows of at most 128
objects and 8 MiB. A single larger valid object is flushed serially under the
existing 64 MiB payload cap. Worktree input buffers, the current encoder buffer,
worker verification buffers and bookkeeping are additional; this is not an RSS
cap or a streaming worktree scan.

`ObjectBackend::max_concurrent_writes` defaults to one. Filesystem providers
suggest four; SQLite, replicas and custom providers retain the serial default
unless explicitly implemented otherwise. The core caps workers at eight and
available CPUs and only starts scoped workers for 32 or more distinct objects.
CPU/cgroup discovery is reused across the windows of one operation. This bound
is per publisher, not a global limit across independent agent workspaces.

Buffers are hash/envelope validated before provider I/O. Equal IDs within a
window share one physical publication, and ID order improves fanout locality.
Each publication retains its normal conditional create, sync and verified live
readback. Every worker is joined on error/panic before returning. All blob
windows must finish before constructing the native root/changeset or publishing
the ref. Dropping the publisher never implicitly flushes pending work. Failures
can leave immutable orphans; the ref stays unchanged and retry is idempotent.

The filesystem sync protocol is unchanged. This is not crash/power-loss
certification, a multi-agent throughput guarantee, or a universal speed claim.

## Separate storage roles

1. **Immutable objects:** canonical blobs, manifests, trees and changesets; verify hashes, support conditional create and streaming.
2. **Mutable refs:** durable, linearizable compare-and-swap for branch heads. Object storage alone is not a substitute.
3. **Derived indexes:** disposable/rebuildable path, ancestry, blame and search indexes; can use SQL, embedded KV or search engines.
4. **Caches:** bounded, verified object caches; never authoritative and never silently override hashes.
5. **Operation journal and coordination:** durable transitions, leases and concurrency controls, with explicitly documented failure models.

## Candidate provider families (roadmap, not implemented)

| Role | Provider candidates | Important qualification |
| --- | --- | --- |
| Local objects | POSIX/Windows filesystem, embedded SQLite, RocksDB, redb, LMDB | Filesystem race hardening and power-loss durability vary |
| Remote objects | Amazon S3, Cloudflare R2, Google Cloud Storage, Azure Blob, MinIO, Ceph RGW | Require conditional writes, integrity checks and multipart/range semantics |
| Distributed object bytes | IPFS-compatible content addressing, custom HTTP/gRPC service | Pinning, availability, quotas and trust must be explicit |
| Authoritative refs | PostgreSQL, SQLite single-writer, CockroachDB, FoundationDB, Spanner, etcd | Must implement transactional/linearizable CAS; not all deployment modes qualify |
| Derived indexes | SQLite, PostgreSQL, RocksDB, Elasticsearch/OpenSearch | Indexes are rebuildable; eventual consistency allowed only here |
| Cache | Memory LRU, local disk, Redis/Valkey, regional cache services | Eviction and stale data must not affect correctness |
| Archival | Cold object tiers, tape/export bundles | Retrieval latency and recovery proof required |

Do **not** claim that an arbitrary SQL/NoSQL store can be used interchangeably for every role. Adapter capability negotiation must check durability, CAS, conditional create, size limits, consistency, range reads, encryption, credentials and delete/list semantics before use.

## Portable configuration target

```toml
[storage.objects]
driver = "filesystem"
path = ".loom/objects"

[storage.refs]
driver = "sqlite"
path = ".loom/index.sqlite"

[storage.index]
driver = "sqlite"

[storage.cache]
driver = "memory"
max_bytes = 268435456
```

This is **target configuration syntax**, not an implemented parser. Future profiles may use different providers for each role and compose multi-region mirrors without changing canonical object IDs.

## Implementation gates

- Implement cloud network drivers on the now-connected `ObjectBackend` interface.
- Preserve the local SQLite journal transaction when introducing configurable authoritative refs; standalone SQLite ref CAS is available already.
- Add verified streaming and bounded-memory reads/writes.
- Add a first remote object backend with fault-injection tests (missing objects, stale listings, duplicate writes, partitions and timeouts).
- Add backup/restore, mirror repair and deterministic migration between backends.
- Validate provider contracts with the same conformance suite and 1/10/100/500-agent workloads.
- Never publish a ref until its complete immutable object closure is durable in the selected authoritative object store.

See issue #26. Git fallback is a separate interoperability layer, not a storage backend.

## Reproduce shared-provider validation

```sh
cargo build -p loom-cli
python3 benchmarks/shared_object_stress.py --binary target/debug/loom --backend sqlite --agents 10 --commits 10
python3 benchmarks/shared_object_stress.py --binary target/debug/loom --backend filesystem --agents 10 --commits 10
```

The script starts isolated workspace/ref databases concurrently, all using one object provider. It verifies each acknowledged first-parent history, final snapshot and full `fsck`, then prints JSON counts and elapsed time. It does not exercise shared branch contention, a remote server, injected crashes or cloud latency. SQLite bootstrap retries journal-mode lock upgrades within a bounded deadline; this path has a synchronized twelve-initializer regression test.


## Grouped immutable reads

`Snapshot::read_files` and `query`'s `cat_many` operation use the same configured
provider, without requiring an object migration or an optimization command.
`ObjectReadSession::max_concurrent_reads` defaults to one; a provider can advertise
a larger hint while the core still caps concurrency at eight and available CPUs.
SQLite keeps its single query-only connection. Filesystem sessions suggest two.
On Unix they retain a bounded directory-handle cache and use no-follow relative
opens; Windows retains the verified filesystem path. All returned envelopes are
hash/type checked. Retained handles/cached bytes never establish live durability.
See [framing and resource limits](snapshot-query-protocol.md#native-grouped-file-reads).

An immutable Snapshot also memoizes exact positive/negative file-path answers
inside its existing budget. These answers belong to one pinned root, are shared
by scalar/grouped reads, and never enter the provider's publication or integrity
paths. Cache accounting conservatively includes retained per-path payloads and
both path-key copies, even when content Arcs are shared. A zero budget disables
this layer too; misses still use the configured verified reader. See the
[working-set comparison](../benchmarks/README.md#repeated-immutable-working-sets).

`loom read-pack @REV FILE` can separately prepare an optional local acceleration
file for any configured provider. Only `query --read-pack FILE` uses it. It is
not an `ObjectBackend`, does not advertise durability, and cannot acknowledge
publication or make missing live objects pass closure/fsck. The pack stores
unmodified canonical tree/blob envelopes and verifies each fetched object.
New objects fall back to the configured read session. Retained index bytes are
reported separately from the object cache; pack preparation and duplicated disk
space are explicit costs. See the [read-pack contract](snapshot-query-protocol.md#optional-immutable-read-packs).
