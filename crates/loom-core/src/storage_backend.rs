//! Storage contracts independent of filesystem paths or database products.
//!
//! Implementations must preserve immutable canonical object bytes and must
//! never claim durability or atomic ref publication they cannot guarantee.
use crate::{ObjectId, Result};
use std::io::{Read, Write};
use std::sync::Arc;

/// Read-only immutable object session. It cannot acknowledge publication or
/// establish that an object is still present at its configured storage path.
/// Consumers must verify the canonical envelope and hash on each cache miss.
pub trait ObjectReadSession: Send + Sync {
    fn get(&self, id: ObjectId) -> Result<Vec<u8>>;
    /// Hint for independent immutable reads; callers still impose their own
    /// thread and memory bounds. Serial providers need not opt into parallelism.
    fn max_concurrent_reads(&self) -> usize {
        1
    }
}

/// Operation-scoped durable writer. A session may retain backend resources
/// between bounded publication windows, but every successful call must still
/// be an independently durable transaction with the same conditional-create,
/// validation and rollback contract as [`ObjectBackend::put_batch_if_absent`].
/// It is never evidence for live publication or object closure.
pub trait ObjectWriteSession: Send {
    fn put_batch_if_absent(&mut self, objects: &[(ObjectId, &[u8])]) -> Result<bool>;
}

/// Capability declaration, not a guarantee inferred from a provider name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreCapabilities {
    pub durable_writes: bool,
    pub conditional_create: bool,
    pub streaming_reads: bool,
    pub streaming_writes: bool,
    pub list_objects: bool,
}

/// Immutable content-addressed object storage. Implementations must verify
/// canonical object hashes on read and reject mismatching existing content.
pub trait ObjectBackend: Send + Sync {
    fn capabilities(&self) -> StoreCapabilities;
    fn get(&self, id: ObjectId) -> Result<Vec<u8>>;
    /// Optionally read a bounded publication window from live storage in input
    /// order. Open fresh storage state for every call: retained read sessions or
    /// caches cannot acknowledge publication. No snapshot or connection may be
    /// retained across calls. Return None to request the scalar get fallback.
    ///
    /// The caller supplies a total canonical-byte budget. Implementations must
    /// enforce it before allocating object payloads and return every requested
    /// object, including duplicates. Core independently validates all returned
    /// bytes; a successful provider call alone never acknowledges publication.
    /// Returning Some must satisfy the same presence obligation as exists for
    /// every configured replica, so it may also certify typed object closure.
    /// Multi-copy providers that cannot prove that must retain the None default.
    fn get_batch_live(&self, _ids: &[ObjectId], _max_bytes: usize) -> Result<Option<Vec<Vec<u8>>>> {
        Ok(None)
    }
    fn put_if_absent(&self, id: ObjectId, canonical_envelope: &[u8]) -> Result<()>;
    /// Optionally publish one already bounded window in a single durable
    /// backend transaction. Returning false asks the core to use the scalar
    /// fallback. Implementations must validate every input before writing,
    /// preserve conditional creation, verify all existing rows and roll the
    /// complete window back on any error.
    fn put_batch_if_absent(&self, _objects: &[(ObjectId, &[u8])]) -> Result<bool> {
        Ok(false)
    }
    /// Optionally retain bounded write-side resources for one object publisher.
    /// Sessions must fail closed if their backing storage path is removed or
    /// replaced. The core continues to perform fresh live readback after every
    /// durable window and drops the session when the publisher finishes.
    fn open_write_session(&self) -> Result<Option<Box<dyn ObjectWriteSession>>> {
        Ok(None)
    }
    /// Opt in to independent durable object publications. This is a hint,
    /// not a durability relaxation: every write must finish its normal sync
    /// and verification. The core bounds workers and joins them on failure.
    fn max_concurrent_writes(&self) -> usize {
        1
    }
    fn exists(&self, id: ObjectId) -> Result<bool>;
    /// Optional retained read handle for repeated immutable queries. Never use
    /// this handle for durability, closure validation, recovery or fsck.
    fn open_read_session(&self) -> Result<Option<Arc<dyn ObjectReadSession>>> {
        Ok(None)
    }
    /// Optional complete enumeration for offline integrity checks. Backends
    /// without authoritative listing must report list_objects=false.
    fn list_ids(&self) -> Result<Vec<ObjectId>> {
        Err(crate::Error::Invalid(
            "object backend enumeration is unsupported".into(),
        ))
    }
}

/// Optional streaming extension for large blobs and remote object stores.
/// A successful finish must mean that the verified immutable object has been
/// published according to the backend's documented durability semantics.
pub trait StreamingObjectBackend: ObjectBackend {
    fn read_into(&self, id: ObjectId, destination: &mut dyn Write) -> Result<u64>;
    fn put_from(&self, id: ObjectId, source: &mut dyn Read, max_bytes: u64) -> Result<()>;
}

/// Atomic mutable reference authority. CAS compares the full optional
/// object ID and must be linearizable for a given reference name.
pub trait RefBackend: Send + Sync {
    fn read_ref(&self, name: &str) -> Result<Option<ObjectId>>;
    /// Returns false when the expected old value is no longer current.
    /// Must never overwrite a concurrent writer on CAS failure.
    fn compare_and_swap(
        &self,
        name: &str,
        expected: Option<ObjectId>,
        replacement: Option<ObjectId>,
    ) -> Result<bool>;
}

/// Disposable acceleration layer; never the authority for object existence
/// or mutable refs. Cache misses must not change correctness.
pub trait ObjectCache: Send + Sync {
    fn get_cached(&self, id: ObjectId) -> Result<Option<Vec<u8>>>;
    fn insert_verified(&self, id: ObjectId, canonical_envelope: &[u8]) -> Result<()>;
    fn invalidate(&self, id: ObjectId) -> Result<()>;
}

/// Storage roles are deliberately separate: an object backend does not
/// automatically qualify as a transactional ref authority.
pub struct StorageLayout<O: ObjectBackend, R: RefBackend> {
    pub objects: O,
    pub refs: R,
}
