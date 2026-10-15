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
        if !self.dir.exists() { return Ok(out); }
        for dir in fs::read_dir(&self.dir)? {
            let dir = dir?;
            if !dir.file_type()?.is_dir() { continue; }
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
