//! Ephemeral, provider-neutral object storage for tests and short-lived agents.
//! Never advertise durability: process exit loses all objects.
use crate::format::{decode_object, object_id};
use crate::storage_backend::{ObjectBackend, StoreCapabilities};
use crate::{Error, ObjectId, Result};
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Default)]
pub struct MemoryObjectBackend {
    objects: RwLock<HashMap<ObjectId, Vec<u8>>>,
}

impl MemoryObjectBackend {
    pub fn new() -> Self { Self::default() }
}

impl ObjectBackend for MemoryObjectBackend {
    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            durable_writes: false,
            conditional_create: true,
            streaming_reads: false,
            streaming_writes: false,
            list_objects: false,
        }
    }

    fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
        let guard = self.objects.read().map_err(|_| Error::Invalid("memory object store lock poisoned".into()))?;
        let bytes = guard.get(&id).ok_or_else(|| Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound, format!("missing object {id}"),
        )))?;
        if object_id(bytes) != id {
            return Err(Error::Invalid(format!("memory object hash mismatch: {id}")));
        }
        decode_object(bytes)?;
        Ok(bytes.clone())
    }

    fn put_if_absent(&self, id: ObjectId, canonical_envelope: &[u8]) -> Result<()> {
        if object_id(canonical_envelope) != id {
            return Err(Error::Invalid("object ID does not match canonical envelope".into()));
        }
        decode_object(canonical_envelope)?;
        let mut guard = self.objects.write().map_err(|_| Error::Invalid("memory object store lock poisoned".into()))?;
        match guard.get(&id) {
            Some(existing) if existing != canonical_envelope =>
                Err(Error::Invalid("immutable object ID collision or corrupt existing content".into())),
            Some(_) => Ok(()),
            None => { guard.insert(id, canonical_envelope.to_vec()); Ok(()) }
        }
    }

    fn exists(&self, id: ObjectId) -> Result<bool> {
        let guard = self.objects.read().map_err(|_| Error::Invalid("memory object store lock poisoned".into()))?;
        match guard.get(&id) {
            Some(bytes) => {
                if object_id(bytes) != id { return Err(Error::Invalid("memory object hash mismatch".into())); }
                decode_object(bytes)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{encode_object, BLOB};
    use std::sync::Arc;

    #[test]
    fn rejects_wrong_ids_and_preserves_immutable_objects() {
        let store = MemoryObjectBackend::new();
        let bytes = encode_object(BLOB, b"hello").unwrap();
        let id = object_id(&bytes);
        assert!(!store.capabilities().durable_writes);
        assert!(!store.exists(id).unwrap());
        assert!(store.put_if_absent(ObjectId([0; 32]), &bytes).is_err());
        store.put_if_absent(id, &bytes).unwrap();
        store.put_if_absent(id, &bytes).unwrap();
        assert_eq!(store.get(id).unwrap(), bytes);
    }

    #[test]
    fn concurrent_writers_publish_same_immutable_bytes() {
        let store = Arc::new(MemoryObjectBackend::new());
        let bytes = encode_object(BLOB, b"concurrent").unwrap();
        let id = object_id(&bytes);
        let threads: Vec<_> = (0..8).map(|_| {
            let store = Arc::clone(&store);
            let bytes = bytes.clone();
            std::thread::spawn(move || store.put_if_absent(id, &bytes).unwrap())
        }).collect();
        for thread in threads { thread.join().unwrap(); }
        assert_eq!(store.get(id).unwrap(), bytes);
    }
}
