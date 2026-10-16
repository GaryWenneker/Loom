use crate::format::{decode_object, encode_object, object_id, ObjectId};
use crate::{Error, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NONCE: AtomicU64 = AtomicU64::new(0);

pub struct FilesystemObjectBackend { dir: PathBuf }

impl FilesystemObjectBackend {
    /// Open an explicit object directory. Repository metadata stays local.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(path.as_ref())?;
        let dir = fs::canonicalize(path.as_ref())?;
        let metadata = fs::symlink_metadata(path.as_ref())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(Error::Invalid("object directory must be a real directory".into()));
        }
        Ok(Self { dir })
    }

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
                Ok(()) => {
                    // Drop the temporary alias before any other writer can
                    // validate the newly published object. Other writers may
                    // still observe the short publication window.
                    fs::remove_file(&tmp)?;
                },
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
        let cleanup_result = if tmp.exists() { fs::remove_file(&tmp) } else { Ok(()) };
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
        let file = fs::File::open(&path)?;
        let open_metadata = file.metadata()?;
        // Hard-link publication briefly creates two names for one inode.
        // Give a concurrent publisher a bounded opportunity to unlink its
        // temporary name, but never accept a persistently multiply-linked
        // object. This is a liveness concession, not a race-free security API.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            for attempt in 0..100 {
                if file.metadata()?.nlink() == 1 { break; }
                if attempt == 99 {
                    return Err(Error::Invalid(format!("object has multiple hard links: {id}")));
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
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

// The first real provider implementation of the pluggable object contract.
// Keep canonical envelope validation at the backend boundary, not at callers.
impl crate::storage_backend::ObjectBackend for FilesystemObjectBackend {
    fn capabilities(&self) -> crate::storage_backend::StoreCapabilities {
        crate::storage_backend::StoreCapabilities {
            durable_writes: true,
            conditional_create: true,
            streaming_reads: false,
            streaming_writes: false,
            list_objects: true,
        }
    }

    fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
        FilesystemObjectBackend::get(self, id)
    }

    fn put_if_absent(&self, id: ObjectId, canonical_envelope: &[u8]) -> Result<()> {
        if object_id(canonical_envelope) != id {
            return Err(Error::Invalid("object ID does not match canonical envelope".into()));
        }
        let (kind, payload) = decode_object(canonical_envelope)?;
        let actual = FilesystemObjectBackend::put(self, kind, payload)?;
        if actual != id {
            return Err(Error::Invalid("backend published unexpected object ID".into()));
        }
        Ok(())
    }

    fn list_ids(&self) -> Result<Vec<ObjectId>> {
        self.ids()
    }

    fn exists(&self, id: ObjectId) -> Result<bool> {
        match FilesystemObjectBackend::get(self, id) {
            Ok(_) => Ok(true),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
}

/// Repository-facing verified facade. Tree and history code never chooses a provider.
pub(crate) struct ObjectStore {
    backend: std::sync::Arc<dyn crate::storage_backend::ObjectBackend>,
    external: bool,
    #[cfg(test)]
    local_dir: Option<PathBuf>,
}

impl ObjectStore {
    pub fn new(meta: &Path) -> Self {
        let dir = meta.join("objects");
        Self {
            backend: std::sync::Arc::new(FilesystemObjectBackend { dir: dir.clone() }),
            external: false,
            #[cfg(test)]
            local_dir: Some(dir),
        }
    }

    pub fn with_backend(backend: std::sync::Arc<dyn crate::storage_backend::ObjectBackend>) -> Result<Self> {
        let capabilities = backend.capabilities();
        if !capabilities.durable_writes || !capabilities.conditional_create {
            return Err(Error::Invalid(
                "repository object backend requires durable writes and conditional create".into(),
            ));
        }
        Ok(Self {
            backend,
            external: true,
            #[cfg(test)]
            local_dir: None,
        })
    }

    pub fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
        let bytes = self.backend.get(id)?;
        if object_id(&bytes) != id {
            return Err(Error::Invalid(format!("object backend returned a hash mismatch: {id}")));
        }
        decode_object(&bytes)?;
        Ok(bytes)
    }

    pub fn put(&self, kind: u8, payload: &[u8]) -> Result<ObjectId> {
        let bytes = encode_object(kind, payload)?;
        let id = object_id(&bytes);
        self.backend.put_if_absent(id, &bytes)?;
        if self.get(id)? != bytes {
            return Err(Error::Invalid(format!("object backend failed publication verification: {id}")));
        }
        Ok(id)
    }

    pub fn ids(&self) -> Result<Vec<ObjectId>> {
        if !self.backend.capabilities().list_objects {
            return Err(Error::Invalid("object backend does not support full fsck enumeration".into()));
        }
        let mut ids = self.backend.list_ids()?;
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }

    /// External providers must acknowledge the complete typed immutable
    /// closure before local refs can make it authoritative. In particular,
    /// replicated providers require presence on every configured replica.
    pub fn verify_external_closure(&self, roots: &[(ObjectId, u8)]) -> Result<()> {
        if !self.external { return Ok(()); }
        let mut pending = roots.to_vec();
        let mut checked = std::collections::HashSet::new();
        while let Some((id, expected)) = pending.pop() {
            if !checked.insert((id, expected)) { continue; }
            if checked.len() > 1_000_000 {
                return Err(Error::Invalid("external object closure exceeds verification limit".into()));
            }
            if !self.backend.exists(id)? {
                return Err(Error::Invalid(format!("object backend has an incomplete closure: {id}")));
            }
            let bytes = self.get(id)?;
            let (kind, _) = decode_object(&bytes)?;
            if kind != expected {
                return Err(Error::Invalid(format!("object backend closure type mismatch: {id}")));
            }
            match kind {
                crate::format::CHANGESET => {
                    let changeset = crate::Changeset::decode(&bytes)?;
                    pending.extend(changeset.parents.into_iter().map(|p| (p, crate::format::CHANGESET)));
                    pending.extend(changeset.changes.into_iter().filter_map(|c| c.blob).map(|b| (b, crate::format::BLOB)));
                }
                crate::format::TREE => {
                    let tree = crate::NativeTree::decode(&bytes)?;
                    pending.extend(tree.entries.into_iter().map(|e| {
                        (e.object, match e.kind {
                            crate::TreeEntryKind::File => crate::format::BLOB,
                            crate::TreeEntryKind::Directory => crate::format::TREE,
                        })
                    }));
                }
                _ => {},
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn path_for_test(&self, id: ObjectId) -> PathBuf {
        let hex = id.to_string();
        self.local_dir.as_ref().expect("filesystem test backend").join(&hex[..2]).join(&hex[2..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::BLOB;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, FilesystemObjectBackend) {
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        fs::create_dir(&meta).unwrap();
        fs::create_dir(meta.join("objects")).unwrap();
        let store = FilesystemObjectBackend::open(meta.join("objects")).unwrap();
        (dir, store)
    }

    #[test]
    fn filesystem_implements_portable_object_backend_contract() {
        use crate::storage_backend::ObjectBackend;
        let (_dir, store) = store();
        let bytes = encode_object(BLOB, b"portable-object").unwrap();
        let id = object_id(&bytes);
        assert!(!ObjectBackend::exists(&store, id).unwrap());
        ObjectBackend::put_if_absent(&store, id, &bytes).unwrap();
        ObjectBackend::put_if_absent(&store, id, &bytes).unwrap();
        assert!(ObjectBackend::exists(&store, id).unwrap());
        assert_eq!(ObjectBackend::get(&store, id).unwrap(), bytes);
        assert!(store.capabilities().conditional_create);
        assert!(ObjectBackend::put_if_absent(&store, ObjectId([0; 32]), &bytes).is_err());
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
    fn reject_hardlinked_object_aliases_without_mutating_them() {
        let (dir, store) = store();
        let id = store.put(BLOB, b"protected").unwrap();
        let alias = dir.path().join("external-alias");
        fs::hard_link(store.path(id), &alias).unwrap();
        assert!(store.get(id).is_err());
        assert!(store.put(BLOB, b"protected").is_err());
        assert_eq!(fs::read(&alias).unwrap(), encode_object(BLOB, b"protected").unwrap());
        fs::remove_file(alias).unwrap();
        assert!(store.get(id).is_ok());
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
