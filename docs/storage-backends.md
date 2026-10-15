# Pluggable storage architecture

## Current implementation

Today Loom stores immutable object envelopes in filesystem fanout directories under `.loom/objects`. SQLite WAL stores refs, staging and derived indexes. These are the **only implemented persistent backends**. This change introduces provider-neutral Rust contracts, but does not yet connect the existing repository engine to those contracts or ship cloud/database drivers.

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

- Integrate `ObjectBackend` with the current filesystem store, then migrate repository call sites behind the interface.
- Integrate `RefBackend` with SQLite using transactionally correct expected-value CAS.
- Add verified streaming and bounded-memory reads/writes.
- Add a first remote object backend with fault-injection tests (missing objects, stale listings, duplicate writes, partitions and timeouts).
- Add backup/restore, mirror repair and deterministic migration between backends.
- Validate provider contracts with the same conformance suite and 1/10/100/500-agent workloads.
- Never publish a ref until its complete immutable object closure is durable in the selected authoritative object store.

See issue #26. Git fallback is a separate interoperability layer, not a storage backend.
