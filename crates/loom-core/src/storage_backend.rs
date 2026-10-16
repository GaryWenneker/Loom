//! Storage contracts independent of filesystem paths or database products.
//!
//! Implementations must preserve immutable canonical object bytes and must
//! never claim durability or atomic ref publication they cannot guarantee.
use crate::{ObjectId, Result};
use std::io::{Read, Write};

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
    fn put_if_absent(&self, id: ObjectId, canonical_envelope: &[u8]) -> Result<()>;
    fn exists(&self, id: ObjectId) -> Result<bool>;
    /// Optional complete enumeration for offline integrity checks. Backends
    /// without authoritative listing must report list_objects=false.
    fn list_ids(&self) -> Result<Vec<ObjectId>> {
        Err(crate::Error::Invalid("object backend enumeration is unsupported".into()))
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
