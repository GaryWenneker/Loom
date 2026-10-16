//! Read-only Unix sessions pin directory handles, never publication authority.
use crate::format::{decode_object, object_id, MAX_OBJECT_SIZE};
use crate::storage_backend::ObjectReadSession;
use crate::{Error, ObjectId, Result};
use rustix::fs::{open, openat, Mode, OFlags};
use std::{
    collections::VecDeque,
    fs::File,
    io::Read,
    os::unix::fs::MetadataExt,
    path::Path,
    sync::{Arc, Mutex},
};

const MAX_OPEN_DIRECTORIES: usize = 64;
struct Directories {
    slots: Vec<Option<Arc<File>>>,
    order: VecDeque<usize>,
}

pub(crate) struct Reader {
    base: File,
    fanout: Mutex<Directories>,
}
impl Reader {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let base = File::from(
            open(
                path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        );
        Ok(Self {
            base,
            fanout: Mutex::new(Directories {
                slots: vec![None; 256],
                order: VecDeque::new(),
            }),
        })
    }
    fn directory(&self, id: ObjectId, hex: &str) -> Result<Arc<File>> {
        let index = id.0[0] as usize;
        if let Some(dir) = self
            .fanout
            .lock()
            .map_err(|_| Error::Invalid("object directory cache poisoned".into()))?
            .slots[index]
            .clone()
        {
            return Ok(dir);
        }
        let dir = Arc::new(File::from(
            openat(
                &self.base,
                &hex[..2],
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        ));
        let mut cache = self
            .fanout
            .lock()
            .map_err(|_| Error::Invalid("object directory cache poisoned".into()))?;
        if let Some(existing) = &cache.slots[index] {
            return Ok(existing.clone());
        }
        if cache.order.len() == MAX_OPEN_DIRECTORIES {
            if let Some(oldest) = cache.order.pop_front() {
                cache.slots[oldest] = None;
            }
        }
        cache.order.push_back(index);
        cache.slots[index] = Some(dir.clone());
        Ok(dir)
    }
}
impl ObjectReadSession for Reader {
    fn max_concurrent_reads(&self) -> usize {
        2
    }
    fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
        let hex = id.to_string();
        let directory = self.directory(id, &hex)?;
        // NONBLOCK prevents a substituted FIFO from blocking before
        // fstat can reject non-regular content. NOFOLLOW rejects leaf symlinks.
        let file = File::from(
            openat(
                &*directory,
                &hex[2..],
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        );
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > (MAX_OBJECT_SIZE + 15) as u64 {
            return Err(Error::Invalid(format!(
                "invalid snapshot object file: {id}"
            )));
        }
        if metadata.nlink() != 1 {
            // Same bounded hard-link publication window as the live provider.
            for attempt in 0..100 {
                if file.metadata()?.nlink() == 1 {
                    break;
                }
                if attempt == 99 {
                    return Err(Error::Invalid(format!(
                        "object has multiple hard links: {id}"
                    )));
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize + 1);
        file.take((MAX_OBJECT_SIZE + 16) as u64)
            .read_to_end(&mut bytes)?;
        if object_id(&bytes) != id {
            return Err(Error::Invalid(format!(
                "object read session hash mismatch: {id}"
            )));
        }
        decode_object(&bytes)?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{encode_object, BLOB};
    use crate::{storage_backend::ObjectBackend, FilesystemObjectBackend};
    use std::os::unix::fs::symlink;

    fn put(backend: &FilesystemObjectBackend, payload: &[u8]) -> (ObjectId, Vec<u8>) {
        let bytes = encode_object(BLOB, payload).unwrap();
        let id = object_id(&bytes);
        backend.put_if_absent(id, &bytes).unwrap();
        (id, bytes)
    }
    #[test]
    fn directory_eviction_keeps_reads_correct_and_descriptors_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FilesystemObjectBackend::open(dir.path()).unwrap();
        let objects: Vec<_> = (0..160)
            .map(|n| put(&backend, format!("object {n}").as_bytes()))
            .collect();
        let reader = Reader::open(dir.path()).unwrap();
        for _ in 0..2 {
            for (id, bytes) in &objects {
                assert_eq!(reader.get(*id).unwrap(), *bytes);
            }
        }
        let directories = reader.fanout.lock().unwrap();
        assert_eq!(directories.order.len(), MAX_OPEN_DIRECTORIES);
        assert_eq!(
            directories.slots.iter().flatten().count(),
            MAX_OPEN_DIRECTORIES
        );
    }
    #[test]
    fn pinned_directory_never_follows_a_substituted_fanout_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects");
        let backend = FilesystemObjectBackend::open(&path).unwrap();
        let (id, bytes) = put(&backend, b"pinned");
        let reader = Reader::open(&path).unwrap();
        assert_eq!(reader.get(id).unwrap(), bytes);
        let hex = id.to_string();
        let parent = path.join(&hex[..2]);
        let saved = dir.path().join("saved");
        std::fs::rename(&parent, &saved).unwrap();
        symlink(&saved, &parent).unwrap();
        assert_eq!(reader.get(id).unwrap(), bytes);
        assert!(backend.get(id).is_err());
        assert!(Reader::open(&path).unwrap().get(id).is_err());
    }
    #[test]
    fn reader_rejects_symlinks_hardlinks_fifos_and_oversized_content() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FilesystemObjectBackend::open(dir.path()).unwrap();
        let (id, bytes) = put(&backend, b"verified");
        let hex = id.to_string();
        let path = dir.path().join(&hex[..2]).join(&hex[2..]);
        let reader = Reader::open(dir.path()).unwrap();
        let alias = dir.path().join("alias");
        std::fs::rename(&path, &alias).unwrap();
        symlink(&alias, &path).unwrap();
        assert!(reader.get(id).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::hard_link(&alias, &path).unwrap();
        assert!(reader.get(id).is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success());
        assert!(reader.get(id).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, bytes).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len((MAX_OBJECT_SIZE + 16) as u64)
            .unwrap();
        assert!(reader.get(id).is_err());
    }
}
