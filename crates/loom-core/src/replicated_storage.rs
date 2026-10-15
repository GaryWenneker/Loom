//! Synchronous verified replication across independent immutable object stores.
//! No distributed ref authority is implied by object replication.
use crate::format::{decode_object, object_id};
use crate::storage_backend::{ObjectBackend, StoreCapabilities};
use crate::{Error, ObjectId, Result};
use std::sync::Arc;

/// All replicas must acknowledge publication before success. A failed write
/// may leave immutable objects on a subset of replicas; retry is idempotent.
pub struct ReplicatedObjectBackend {
    replicas: Vec<Arc<dyn ObjectBackend>>,
}

impl ReplicatedObjectBackend {
    pub fn new(replicas: Vec<Arc<dyn ObjectBackend>>) -> Result<Self> {
        if replicas.is_empty() {
            return Err(Error::Invalid("replicated storage requires at least one backend".into()));
        }
        if replicas.iter().any(|r| {
            let c = r.capabilities();
            !c.durable_writes || !c.conditional_create
        }) {
            return Err(Error::Invalid(
                "replicated storage requires durable conditional-create backends".into(),
            ));
        }
        Ok(Self { replicas })
    }

    pub fn replica_count(&self) -> usize { self.replicas.len() }
}

impl ObjectBackend for ReplicatedObjectBackend {
    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            durable_writes: true,
            conditional_create: true,
            streaming_reads: false,
            streaming_writes: false,
            list_objects: false,
        }
    }

    fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
        let mut missing = false;
        for replica in &self.replicas {
            match replica.get(id) {
                Ok(bytes) => {
                    if object_id(&bytes) != id {
                        return Err(Error::Invalid(format!("replica returned invalid hash for {id}")));
                    }
                    decode_object(&bytes)?;
                    return Ok(bytes);
                }
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing = true;
                }
                Err(error) => return Err(error),
            }
        }
        if missing {
            Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound, format!("object {id} absent from all replicas"),
            )))
        } else {
            Err(Error::Invalid("no replicas available".into()))
        }
    }

    fn put_if_absent(&self, id: ObjectId, canonical_envelope: &[u8]) -> Result<()> {
        if object_id(canonical_envelope) != id {
            return Err(Error::Invalid("object ID does not match canonical envelope".into()));
        }
        decode_object(canonical_envelope)?;
        for replica in &self.replicas {
            replica.put_if_absent(id, canonical_envelope)?;
            // A backend must not acknowledge bytes that cannot be verified.
            if replica.get(id)? != canonical_envelope {
                return Err(Error::Invalid(format!("replica failed read-after-write verification for {id}")));
            }
        }
        Ok(())
    }

    fn exists(&self, id: ObjectId) -> Result<bool> {
        // Existence on only one replica is insufficient to promise a durable
        // fully replicated object closure for later ref publication.
        for replica in &self.replicas {
            if !replica.exists(id)? { return Ok(false); }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{encode_object, BLOB};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeDurable {
        bytes: Mutex<HashMap<ObjectId, Vec<u8>>>,
        fail: AtomicBool,
    }
    impl ObjectBackend for FakeDurable {
        fn capabilities(&self) -> StoreCapabilities {
            StoreCapabilities { durable_writes: true, conditional_create: true,
                streaming_reads: false, streaming_writes: false, list_objects: false }
        }
        fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
            self.bytes.lock().unwrap().get(&id).cloned().ok_or_else(|| {
                Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"))
            })
        }
        fn put_if_absent(&self, id: ObjectId, bytes: &[u8]) -> Result<()> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(Error::Io(std::io::Error::other("injected outage")));
            }
            self.bytes.lock().unwrap().entry(id).or_insert_with(|| bytes.to_vec());
            Ok(())
        }
        fn exists(&self, id: ObjectId) -> Result<bool> {
            Ok(self.bytes.lock().unwrap().contains_key(&id))
        }
    }

    #[test]
    fn all_ack_and_retry_after_partial_failure() {
        let first = Arc::new(FakeDurable::default());
        let second = Arc::new(FakeDurable::default());
        second.fail.store(true, Ordering::SeqCst);
        let store = ReplicatedObjectBackend::new(vec![first.clone(), second.clone()]).unwrap();
        let bytes = encode_object(BLOB, b"replication").unwrap();
        let id = object_id(&bytes);
        assert!(store.put_if_absent(id, &bytes).is_err());
        assert!(!store.exists(id).unwrap());
        second.fail.store(false, Ordering::SeqCst);
        store.put_if_absent(id, &bytes).unwrap();
        assert!(store.exists(id).unwrap());
        assert_eq!(store.get(id).unwrap(), bytes);
    }

    #[test]
    fn rejects_non_durable_replica() {
        struct Volatile;
        impl ObjectBackend for Volatile {
            fn capabilities(&self) -> StoreCapabilities {
                StoreCapabilities { durable_writes: false, conditional_create: true,
                    streaming_reads: false, streaming_writes: false, list_objects: false }
            }
            fn get(&self, _: ObjectId) -> Result<Vec<u8>> { unreachable!() }
            fn put_if_absent(&self, _: ObjectId, _: &[u8]) -> Result<()> { unreachable!() }
            fn exists(&self, _: ObjectId) -> Result<bool> { unreachable!() }
        }
        assert!(ReplicatedObjectBackend::new(vec![Arc::new(Volatile)]).is_err());
    }
}
