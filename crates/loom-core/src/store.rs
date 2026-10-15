use crate::format::{decode_object, encode_object, object_id, ObjectId};
use crate::{Error, Result};
use std::fs::{self, File, OpenOptions};
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

    pub fn put(&self, kind: u8, payload: &[u8]) -> Result<ObjectId> {
        let data = encode_object(kind, payload)?;
        let id = object_id(&data);
        let path = self.path(id);
        if path.exists() {
            self.get(id)?; // existing hash must verify; never trust an existing filename
            return Ok(id);
        }
        let parent = path.parent().ok_or_else(|| Error::Invalid("invalid object path".into()))?;
        fs::create_dir_all(parent)?;
        let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(".tmp-{}-{nonce}", std::process::id()));
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        let result = (|| -> Result<()> {
            f.write_all(&data)?;
            f.sync_all()?;
            drop(f);
            match fs::rename(&tmp, &path) {
                Ok(()) => {},
                Err(e) if path.exists() => {
                    fs::remove_file(&tmp)?;
                    self.get(id)?;
                    let _ = e;
                }
                Err(e) => return Err(Error::Io(e)),
            }
            #[cfg(unix)]
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() { let _ = fs::remove_file(&tmp); }
        result?;
        Ok(id)
    }

    pub fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
        let path = self.path(id);
        let bytes = fs::read(path)?;
        if object_id(&bytes) != id {
            return Err(Error::Invalid(format!("object hash mismatch: {id}")));
        }
        decode_object(&bytes)?;
        Ok(bytes)
    }

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
