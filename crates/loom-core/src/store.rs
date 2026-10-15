use crate::format::{decode_object, encode_object, object_id, ObjectId};
use crate::{Error, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NONCE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct ObjectStore { dir: PathBuf }

impl ObjectStore {
    pub fn new(meta: &Path) -> Self { Self { dir: meta.join("objects") } }

    fn path(&self, id: ObjectId) -> PathBuf {
        let hex = id.to_string();
        self.dir.join(&hex[..2]).join(&hex[2..])
    }

    /// Verify the object directory components before following paths. These
    /// checks reject pre-existing symlink substitutions, but do not claim
    /// complete protection against an adversary concurrently replacing dirs.
    fn checked_parent(&self, id: ObjectId) -> Result<PathBuf> {
        let path = self.path(id);
        let parent = path.parent()
            .ok_or_else(|| Error::Invalid("object path has no parent".into()))?;
        for dir in [&self.dir, parent] {
            let metadata = fs::symlink_metadata(dir)?;
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err(Error::Invalid(format!(
                    "object directory is not a real directory: {}", dir.display()
                )));
            }
        }
        Ok(parent.to_path_buf())
    }

    /// Write immutable object content with create-if-absent publication.
    /// A same-directory hard link is atomic and MUST NOT replace an existing
    /// object; when unsupported by the filesystem, fail instead of overwriting.
    pub fn put(&self, kind: u8, payload: &[u8]) -> Result<ObjectId> {
        let data = encode_object(kind, payload)?;
        let id = object_id(&data);
        let path = self.path(id);
        // Validate object base before creating any fanout directories.
        let base = fs::symlink_metadata(&self.dir)?;
        if !base.file_type().is_dir() || base.file_type().is_symlink() {
            return Err(Error::Invalid("invalid object storage directory".into()));
        }
        let parent = path.parent().ok_or_else(||
            Error::Invalid("object path has no parent".into())
        )?;
        fs::create_dir_all(parent)?;
        self.checked_parent(id)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => { self.get(id)?; return Ok(id); }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.into()),
        }

        let mut temp_file = None;
        for _ in 0..32 {
            let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
            let candidate = parent.join(format!(".tmp-{}-{nonce}", std::process::id()));
            match OpenOptions::new().write(true).create_new(true).open(&candidate) {
                Ok(file) => { temp_file = Some((candidate, file)); break; }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {},
                Err(e) => return Err(e.into()),
            }
        }
        let (tmp, mut file) = temp_file.ok_or_else(||
            Error::Conflict("could not allocate unique object temp file".into())
        )?;
        let publish_result = (|| -> Result<()> {
            file.write_all(&data)?;
            file.sync_all()?;
            drop(file);
            self.checked_parent(id)?;
            match fs::hard_link(&tmp, &path) {
                Ok(()) => {},
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Another writer published first. Never modify their inode.
                    self.get(id)?;
                },
                Err(e) => return Err(e.into()),
            }
            #[cfg(unix)]
            fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        let cleanup_result = fs::remove_file(&tmp);
        publish_result?;
        cleanup_result?;
        Ok(id)
    }

    pub fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
        use std::io::Read;
        self.checked_parent(id)?;
        let path = self.path(id);
        let link_metadata = fs::symlink_metadata(&path)?;
        if !link_metadata.file_type().is_file() || link_metadata.file_type().is_symlink() {
            return Err(Error::Invalid(format!("object is not a regular file: {id}")));
        }
        if link_metadata.len() > (crate::format::MAX_OBJECT_SIZE + 15) as u64 {
            return Err(Error::Invalid(format!("object exceeds v1 read limit: {id}")));
        }
        let mut file = fs::File::open(&path)?;
        let open_metadata = file.metadata()?;
        if !open_metadata.is_file() || open_metadata.len() > (crate::format::MAX_OBJECT_SIZE + 15) as u64 {
            return Err(Error::Invalid(format!("object changed during open: {id}")));
        }
        let mut bytes = Vec::new();
        file.take((crate::format::MAX_OBJECT_SIZE + 16) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > crate::format::MAX_OBJECT_SIZE + 15 {
            return Err(Error::Invalid(format!("object exceeds v1 read limit: {id}")));
        }
        if object_id(&bytes) != id {
            return Err(Error::Invalid(format!("object hash mismatch: {id}")));
        }
        decode_object(&bytes)?;
        Ok(bytes)
    }

    #[cfg(test)]
    pub(crate) fn path_for_test(&self, id: ObjectId) -> PathBuf { self.path(id) }

    pub fn ids(&self) -> Result<Vec<ObjectId>> {
        let mut out = Vec::new();
        match fs::symlink_metadata(&self.dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
            Ok(meta) if !meta.file_type().is_dir() || meta.file_type().is_symlink() => {
                return Err(Error::Invalid("object storage directory is not a real directory".into()));
            }
            Ok(_) => {},
        }
        for dir in fs::read_dir(&self.dir)? {
            let dir = dir?;
            if !dir.file_type()?.is_dir() {
                return Err(Error::Invalid("unexpected non-directory object fanout".into()));
            }
            let prefix = dir.file_name().into_string().map_err(|_| Error::Invalid("invalid object prefix".into()))?;
            if prefix.len() != 2 { return Err(Error::Invalid("invalid object directory name".into())); }
            for entry in fs::read_dir(dir.path())? {
                let entry = entry?;
                let name = entry.file_name().into_string().map_err(|_| Error::Invalid("invalid object filename".into()))?;
                if name.starts_with(".tmp-") { continue; }
                if !entry.file_type()?.is_file() { return Err(Error::Invalid("object is not a regular file".into())); }
                out.push(ObjectId::parse(&format!("{prefix}{name}"))?);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::BLOB;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, ObjectStore) {
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        fs::create_dir(&meta).unwrap();
        fs::create_dir(meta.join("objects")).unwrap();
        let store = ObjectStore::new(&meta);
        (dir, store)
    }

    #[test]
    fn immutable_publication_is_idempotent_with_parallel_writers() {
        let (_dir, store) = store();
        let store = Arc::new(store);
        let handles: Vec<_> = (0..12)
            .map(|_| {
                let store = Arc::clone(&store);
                std::thread::spawn(move || store.put(BLOB, b"same payload"))
            })
            .collect();
        let ids: Vec<_> = handles.into_iter().map(|h| h.join().unwrap().unwrap()).collect();
        assert!(ids.iter().all(|id| *id == ids[0]));
        assert_eq!(store.ids().unwrap(), vec![ids[0]]);
        assert_eq!(store.get(ids[0]).unwrap(),
                   encode_object(BLOB, b"same payload").unwrap());
    }

    #[test]
    fn existing_corrupt_object_cannot_be_silently_replaced() {
        let (_dir, store) = store();
        let id = store.put(BLOB, b"correct bytes").unwrap();
        let path = store.path(id);
        fs::write(&path, b"tampered").unwrap();
        assert!(store.put(BLOB, b"correct bytes").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"tampered");
    }

    #[test]
    fn oversized_object_is_rejected_before_loading() {
        let (_dir, store) = store();
        let id = store.put(BLOB, b"small").unwrap();
        let path = store.path(id);
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len((crate::format::MAX_OBJECT_SIZE + 16) as u64).unwrap();
        assert!(store.get(id).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn reject_object_symlinks_and_symlinked_fanout_directory() {
        use std::os::unix::fs::symlink;
        let (dir, store) = store();
        let id = store.put(BLOB, b"data").unwrap();
        let target = dir.path().join("outside");
        fs::write(&target, b"outside").unwrap();
        let path = store.path(id);
        fs::remove_file(&path).unwrap();
        symlink(&target, &path).unwrap();
        assert!(store.get(id).is_err());
        assert!(store.put(BLOB, b"data").is_err());
        fs::remove_file(&path).unwrap();

        let parent = path.parent().unwrap();
        fs::remove_dir(parent).unwrap();
        symlink(dir.path(), parent).unwrap();
        assert!(store.get(id).is_err());
        assert!(store.put(BLOB, b"data").is_err());
        assert!(store.ids().is_err());
    }
}
