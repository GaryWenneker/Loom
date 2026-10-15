//! Reusable black-box contract tests for object backends.
//! Provider implementations should run these against local emulators or test instances.
use crate::format::{encode_object, object_id, BLOB};
use crate::storage_backend::ObjectBackend;
use crate::{Error, ObjectId, Result};

/// Validate canonical identity, immutable idempotence and missing-object semantics.
pub fn verify_object_backend(backend: &dyn ObjectBackend) -> Result<()> {
    let bytes = encode_object(BLOB, b"loom storage conformance v1")?;
    let id = object_id(&bytes);
    if backend.exists(id)? {
        let existing = backend.get(id)?;
        if existing != bytes {
            return Err(Error::Invalid("conformance fixture ID has unexpected bytes".into()));
        }
    } else {
        backend.put_if_absent(id, &bytes)?;
    }
    backend.put_if_absent(id, &bytes)?;
    if !backend.exists(id)? || backend.get(id)? != bytes {
        return Err(Error::Invalid("object backend failed immutable round-trip".into()));
    }
    let wrong = ObjectId([0; 32]);
    if wrong != id && backend.put_if_absent(wrong, &bytes).is_ok() {
        return Err(Error::Invalid("object backend accepted mismatched ID".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ObjectStore;

    #[test]
    fn filesystem_passes_object_conformance() {
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        std::fs::create_dir_all(meta.join("objects")).unwrap();
        let store = ObjectStore::new(&meta);
        verify_object_backend(&store).unwrap();
    }
}
