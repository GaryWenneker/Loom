use crate::format::{decode_object, encode_object, object_id, ObjectId};
use crate::{Error, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NONCE: AtomicU64 = AtomicU64::new(0);

pub(crate) const MAX_PUBLICATION_OBJECTS: usize = 128;
pub(crate) const MAX_PUBLICATION_BYTES: usize = 8 * 1024 * 1024;

pub struct FilesystemObjectBackend {
    dir: PathBuf,
}

impl FilesystemObjectBackend {
    /// Open an explicit object directory. Repository metadata stays local.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(path.as_ref())?;
        Self::open_existing(path)
    }

    /// Open without creating storage or changing its contents.
    pub fn open_existing(path: impl AsRef<Path>) -> Result<Self> {
        let dir = fs::canonicalize(path.as_ref())?;
        let metadata = fs::symlink_metadata(path.as_ref())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(Error::Invalid(
                "object directory must be a real directory".into(),
            ));
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
        let parent = path
            .parent()
            .ok_or_else(|| Error::Invalid("object path has no parent".into()))?;
        for dir in [&self.dir, parent] {
            let metadata = fs::symlink_metadata(dir)?;
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err(Error::Invalid(format!(
                    "object directory is not a real directory: {}",
                    dir.display()
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
        let parent = path
            .parent()
            .ok_or_else(|| Error::Invalid("object path has no parent".into()))?;
        fs::create_dir_all(parent)?;
        self.checked_parent(id)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                self.get(id)?;
                return Ok(id);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }

        let mut temp_file = None;
        for _ in 0..32 {
            let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
            let candidate = parent.join(format!(".tmp-{}-{nonce}", std::process::id()));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(file) => {
                    temp_file = Some((candidate, file));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        let (tmp, mut file) = temp_file
            .ok_or_else(|| Error::Conflict("could not allocate unique object temp file".into()))?;
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
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Another writer published first. Never modify their inode.
                    self.get(id)?;
                }
                Err(e) => return Err(e.into()),
            }
            #[cfg(unix)]
            fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        let cleanup_result = if tmp.exists() {
            fs::remove_file(&tmp)
        } else {
            Ok(())
        };
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
            return Err(Error::Invalid(format!(
                "object is not a regular file: {id}"
            )));
        }
        if link_metadata.len() > (crate::format::MAX_OBJECT_SIZE + 15) as u64 {
            return Err(Error::Invalid(format!(
                "object exceeds v1 read limit: {id}"
            )));
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
        if !open_metadata.is_file()
            || open_metadata.len() > (crate::format::MAX_OBJECT_SIZE + 15) as u64
        {
            return Err(Error::Invalid(format!("object changed during open: {id}")));
        }
        let mut bytes = Vec::new();
        file.take((crate::format::MAX_OBJECT_SIZE + 16) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > crate::format::MAX_OBJECT_SIZE + 15 {
            return Err(Error::Invalid(format!(
                "object exceeds v1 read limit: {id}"
            )));
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
                return Err(Error::Invalid(
                    "object storage directory is not a real directory".into(),
                ));
            }
            Ok(_) => {}
        }
        for dir in fs::read_dir(&self.dir)? {
            let dir = dir?;
            if !dir.file_type()?.is_dir() {
                return Err(Error::Invalid(
                    "unexpected non-directory object fanout".into(),
                ));
            }
            let prefix = dir
                .file_name()
                .into_string()
                .map_err(|_| Error::Invalid("invalid object prefix".into()))?;
            if prefix.len() != 2 {
                return Err(Error::Invalid("invalid object directory name".into()));
            }
            for entry in fs::read_dir(dir.path())? {
                let entry = entry?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| Error::Invalid("invalid object filename".into()))?;
                if name.starts_with(".tmp-") {
                    continue;
                }
                if !entry.file_type()?.is_file() {
                    return Err(Error::Invalid("object is not a regular file".into()));
                }
                out.push(ObjectId::parse(&format!("{prefix}{name}"))?);
            }
        }
        Ok(out)
    }
}

// The first real provider implementation of the pluggable object contract.
// Keep canonical envelope validation at the backend boundary, not at callers.
impl crate::storage_backend::ObjectBackend for FilesystemObjectBackend {
    fn max_concurrent_writes(&self) -> usize {
        4
    }
    fn open_read_session(
        &self,
    ) -> Result<Option<std::sync::Arc<dyn crate::storage_backend::ObjectReadSession>>> {
        #[cfg(unix)]
        {
            Ok(Some(std::sync::Arc::new(
                crate::filesystem_read::Reader::open(&self.dir)?,
            )))
        }
        #[cfg(not(unix))]
        {
            struct Reader(FilesystemObjectBackend);
            impl crate::storage_backend::ObjectReadSession for Reader {
                fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
                    self.0.get(id)
                }
                fn max_concurrent_reads(&self) -> usize {
                    2
                }
            }
            Ok(Some(std::sync::Arc::new(Reader(FilesystemObjectBackend {
                dir: self.dir.clone(),
            }))))
        }
    }
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
            return Err(Error::Invalid(
                "object ID does not match canonical envelope".into(),
            ));
        }
        let (kind, payload) = decode_object(canonical_envelope)?;
        let actual = FilesystemObjectBackend::put(self, kind, payload)?;
        if actual != id {
            return Err(Error::Invalid(
                "backend published unexpected object ID".into(),
            ));
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
#[derive(Clone)]
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

    pub fn with_backend(
        backend: std::sync::Arc<dyn crate::storage_backend::ObjectBackend>,
    ) -> Result<Self> {
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
            return Err(Error::Invalid(format!(
                "object backend returned a hash mismatch: {id}"
            )));
        }
        decode_object(&bytes)?;
        Ok(bytes)
    }

    fn live_batch(&self, ids: &[ObjectId], max_bytes: usize) -> Result<Option<Vec<Vec<u8>>>> {
        let Some(objects) = self.backend.get_batch_live(ids, max_bytes)? else {
            return Ok(None);
        };
        if objects.len() != ids.len() {
            return Err(Error::Invalid(
                "object backend returned an incomplete live batch".into(),
            ));
        }
        let mut remaining = max_bytes;
        for (id, bytes) in ids.iter().zip(&objects) {
            remaining = remaining.checked_sub(bytes.len()).ok_or_else(|| {
                Error::Invalid("object backend exceeded live read byte budget".into())
            })?;
            if object_id(bytes) != *id {
                return Err(Error::Invalid(format!(
                    "object backend returned a live batch hash mismatch: {id}"
                )));
            }
            decode_object(bytes)?;
        }
        Ok(Some(objects))
    }

    pub fn read_session(
        &self,
    ) -> Result<std::sync::Arc<dyn crate::storage_backend::ObjectReadSession>> {
        use crate::storage_backend::ObjectReadSession;
        struct VerifiedSession(std::sync::Arc<dyn ObjectReadSession>);
        impl ObjectReadSession for VerifiedSession {
            fn max_concurrent_reads(&self) -> usize {
                self.0.max_concurrent_reads()
            }
            fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
                let bytes = self.0.get(id)?;
                if object_id(&bytes) != id {
                    return Err(Error::Invalid(format!(
                        "object read session returned a hash mismatch: {id}"
                    )));
                }
                decode_object(&bytes)?;
                Ok(bytes)
            }
        }
        struct LiveSession(ObjectStore);
        impl ObjectReadSession for LiveSession {
            fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
                self.0.get(id)
            }
        }
        Ok(match self.backend.open_read_session()? {
            Some(session) => std::sync::Arc::new(VerifiedSession(session)),
            None => std::sync::Arc::new(LiveSession(self.clone())),
        })
    }

    pub fn put(&self, kind: u8, payload: &[u8]) -> Result<ObjectId> {
        let bytes = encode_object(kind, payload)?;
        let id = object_id(&bytes);
        self.publish_canonical(id, &bytes)?;
        Ok(id)
    }

    fn publish_canonical(&self, id: ObjectId, bytes: &[u8]) -> Result<()> {
        self.backend.put_if_absent(id, bytes)?;
        if self.get(id)? != bytes {
            return Err(Error::Invalid(format!(
                "object backend failed publication verification: {id}"
            )));
        }
        Ok(())
    }

    pub(crate) fn publisher(&self) -> ObjectPublisher<'_> {
        ObjectPublisher {
            store: self,
            objects: Vec::new(),
            bytes: 0,
            workers: None,
            writer: None,
            writer_checked: false,
        }
    }

    /// Shared bound for independent live object publications, including
    /// native subtree builders that publish outside the buffered publisher.
    pub(crate) fn max_concurrent_writes(&self) -> usize {
        self.backend.max_concurrent_writes().clamp(1, 8)
    }

    pub fn ids(&self) -> Result<Vec<ObjectId>> {
        if !self.backend.capabilities().list_objects {
            return Err(Error::Invalid(
                "object backend does not support full fsck enumeration".into(),
            ));
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
        if !self.external {
            return Ok(());
        }
        let mut pending = roots.to_vec();
        let mut checked = std::collections::HashSet::new();
        while let Some((id, expected)) = pending.pop() {
            if !checked.insert((id, expected)) {
                continue;
            }
            if checked.len() > 1_000_000 {
                return Err(Error::Invalid(
                    "external object closure exceeds verification limit".into(),
                ));
            }
            let bytes = match self.live_batch(&[id], crate::format::MAX_OBJECT_SIZE + 15)? {
                Some(mut objects) => objects.pop().expect("one checked live object"),
                None => {
                    // Default and replicated providers keep their complete
                    // all-copy existence check before the separate verified get.
                    if !self.backend.exists(id)? {
                        return Err(Error::Invalid(format!(
                            "object backend has an incomplete closure: {id}"
                        )));
                    }
                    self.get(id)?
                }
            };
            let (kind, _) = decode_object(&bytes)?;
            if kind != expected {
                return Err(Error::Invalid(format!(
                    "object backend closure type mismatch: {id}"
                )));
            }
            match kind {
                crate::format::CHANGESET => {
                    let changeset = crate::Changeset::decode(&bytes)?;
                    pending.extend(
                        changeset
                            .parents
                            .into_iter()
                            .map(|p| (p, crate::format::CHANGESET)),
                    );
                    pending.extend(
                        changeset
                            .changes
                            .into_iter()
                            .filter_map(|c| c.blob)
                            .map(|b| (b, crate::format::BLOB)),
                    );
                }
                crate::format::TREE => {
                    let tree = crate::NativeTree::decode(&bytes)?;
                    pending.extend(tree.entries.into_iter().map(|e| {
                        (
                            e.object,
                            match e.kind {
                                crate::TreeEntryKind::File => crate::format::BLOB,
                                crate::TreeEntryKind::Directory => crate::format::TREE,
                            },
                        )
                    }));
                }
                crate::format::BLOB_MANIFEST => {
                    let manifest = crate::PortableBlobManifest::decode(&bytes)?;
                    pending.extend(
                        manifest
                            .chunks
                            .into_iter()
                            .map(|chunk| (chunk, crate::format::BLOB)),
                    );
                }
                crate::format::PORTABLE_TREE => {
                    let tree = crate::PortableTree::decode(&bytes)?;
                    pending.extend(tree.entries.into_iter().map(|entry| {
                        let kind = match entry.kind {
                            crate::PortableTreeEntryKind::File => crate::format::BLOB_MANIFEST,
                            crate::PortableTreeEntryKind::Directory => crate::format::PORTABLE_TREE,
                        };
                        (entry.object, kind)
                    }));
                }
                _ => {}
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn path_for_test(&self, id: ObjectId) -> PathBuf {
        let hex = id.to_string();
        self.local_dir
            .as_ref()
            .expect("filesystem test backend")
            .join(&hex[..2])
            .join(&hex[2..])
    }
}

/// Explicit finish: dropping a failed operation never publishes buffered data.
/// Only this window is bounded; worktree input and backend readback buffers are
/// additional. A single oversized existing-format object runs serially.
pub(crate) struct ObjectPublisher<'a> {
    store: &'a ObjectStore,
    objects: Vec<(ObjectId, Vec<u8>)>,
    bytes: usize,
    workers: Option<usize>,
    writer: Option<Box<dyn crate::storage_backend::ObjectWriteSession>>,
    writer_checked: bool,
}
impl ObjectPublisher<'_> {
    pub fn push(&mut self, id: ObjectId, data: Vec<u8>) -> Result<()> {
        // Validate before flushing even the previous window. Later I/O can
        // only see these sealed canonical buffers through shared references.
        if object_id(&data) != id {
            return Err(Error::Invalid("publication object hash mismatch".into()));
        }
        decode_object(&data)?;
        if self.objects.len() == MAX_PUBLICATION_OBJECTS
            || self.bytes.saturating_add(data.len()) > MAX_PUBLICATION_BYTES
        {
            self.flush()?;
        }
        self.bytes += data.len();
        self.objects.push((id, data));
        if self.bytes > MAX_PUBLICATION_BYTES {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if self.objects.is_empty() {
            return Ok(());
        }
        let mut unique = std::collections::BTreeMap::new();
        for (id, data) in &self.objects {
            if let Some(prior) = unique.insert(*id, data) {
                if prior != data {
                    return Err(Error::Invalid("publication object collision".into()));
                }
            }
        }
        let work: Vec<_> = unique.into_iter().collect();
        let batch: Vec<_> = work
            .iter()
            .map(|(id, data)| (*id, data.as_slice()))
            .collect();
        if !self.writer_checked {
            self.writer = self.store.backend.open_write_session()?;
            self.writer_checked = true;
        }
        let batched = match self.writer.as_mut() {
            Some(writer) => writer.put_batch_if_absent(&batch)?,
            None => self.store.backend.put_batch_if_absent(&batch)?,
        };
        if batched {
            // The provider transaction is complete, but ref publication still
            // requires a fresh live read rather than trusting its acknowledgement.
            let ids: Vec<_> = work.iter().map(|(id, _)| *id).collect();
            let max_bytes = work.iter().map(|(_, data)| data.len()).sum();
            match self.store.live_batch(&ids, max_bytes)? {
                Some(actual) => {
                    for ((id, expected), actual) in work.iter().zip(actual) {
                        if actual != expected.as_slice() {
                            return Err(Error::Invalid(format!(
                                "object backend failed batch publication verification: {id}"
                            )));
                        }
                    }
                }
                None => {
                    for (id, data) in &work {
                        if self.store.get(*id)? != data.as_slice() {
                            return Err(Error::Invalid(format!(
                                "object backend failed batch publication verification: {id}"
                            )));
                        }
                    }
                }
            }
            self.objects.clear();
            self.bytes = 0;
            return Ok(());
        }
        let workers = if work.len() >= 32 {
            *self.workers.get_or_insert_with(|| {
                let hint = self.store.max_concurrent_writes();
                if hint > 1 {
                    hint.min(std::thread::available_parallelism().map_or(1, usize::from))
                } else {
                    1
                }
            })
        } else {
            1
        };
        crate::workers::parallel_map(&work, workers, &|(id, data)| {
            self.store.publish_canonical(*id, data)
        })?;
        self.objects.clear();
        self.bytes = 0;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::BLOB;
    use std::sync::Arc;

    #[derive(Default)]
    struct Recording {
        values: std::sync::Mutex<std::collections::HashMap<ObjectId, Vec<u8>>>,
        puts: std::sync::atomic::AtomicUsize,
        corrupt: std::sync::atomic::AtomicBool,
        threads: std::sync::Mutex<std::collections::HashSet<std::thread::ThreadId>>,
    }
    impl crate::storage_backend::ObjectBackend for Recording {
        fn capabilities(&self) -> crate::storage_backend::StoreCapabilities {
            crate::storage_backend::StoreCapabilities {
                durable_writes: true,
                conditional_create: true,
                streaming_reads: false,
                streaming_writes: false,
                list_objects: false,
            }
        }
        fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
            if self.corrupt.load(Ordering::SeqCst) {
                return encode_object(BLOB, b"wrong readback");
            }
            self.values
                .lock()
                .unwrap()
                .get(&id)
                .cloned()
                .ok_or_else(|| Error::Io(std::io::Error::from(std::io::ErrorKind::NotFound)))
        }
        fn put_if_absent(&self, id: ObjectId, bytes: &[u8]) -> Result<()> {
            self.threads
                .lock()
                .unwrap()
                .insert(std::thread::current().id());
            self.puts.fetch_add(1, Ordering::SeqCst);
            self.values
                .lock()
                .unwrap()
                .entry(id)
                .or_insert_with(|| bytes.to_vec());
            Ok(())
        }
        fn exists(&self, id: ObjectId) -> Result<bool> {
            Ok(self.values.lock().unwrap().contains_key(&id))
        }
    }

    #[test]
    fn publication_windows_bound_memory_deduplicate_and_require_explicit_finish() {
        let backend = Arc::new(Recording::default());
        let store = ObjectStore::with_backend(backend.clone()).unwrap();
        let mut publication = store.publisher();
        for n in 0..MAX_PUBLICATION_OBJECTS {
            let bytes = encode_object(BLOB, format!("unique {n}").as_bytes()).unwrap();
            publication.push(object_id(&bytes), bytes).unwrap();
        }
        assert_eq!(backend.puts.load(Ordering::SeqCst), 0);
        let extra = encode_object(BLOB, b"deferred final object").unwrap();
        publication.push(object_id(&extra), extra).unwrap();
        assert_eq!(backend.puts.load(Ordering::SeqCst), MAX_PUBLICATION_OBJECTS);
        drop(publication);
        assert_eq!(backend.puts.load(Ordering::SeqCst), MAX_PUBLICATION_OBJECTS);

        let mut publication = store.publisher();
        for _ in 0..64 {
            let bytes = encode_object(BLOB, b"shared blob").unwrap();
            publication.push(object_id(&bytes), bytes).unwrap();
        }
        publication.finish().unwrap();
        assert_eq!(
            backend.puts.load(Ordering::SeqCst),
            MAX_PUBLICATION_OBJECTS + 1
        );

        let mut publication = store.publisher();
        for n in 0..2 {
            let bytes = encode_object(BLOB, &vec![n; MAX_PUBLICATION_BYTES / 2]).unwrap();
            publication.push(object_id(&bytes), bytes).unwrap();
            assert!(publication.bytes <= MAX_PUBLICATION_BYTES);
        }
        assert_eq!(
            backend.puts.load(Ordering::SeqCst),
            MAX_PUBLICATION_OBJECTS + 2
        );
        publication.finish().unwrap();
        let bytes = encode_object(BLOB, &vec![7; MAX_PUBLICATION_BYTES + 1]).unwrap();
        let mut publication = store.publisher();
        publication.push(object_id(&bytes), bytes).unwrap();
        assert!(publication.objects.is_empty());
        assert_eq!(
            backend.puts.load(Ordering::SeqCst),
            MAX_PUBLICATION_OBJECTS + 4
        );
        assert_eq!(backend.threads.lock().unwrap().len(), 1);
    }

    #[test]
    fn publication_clamps_provider_hint_and_reuses_cpu_discovery() {
        use crate::storage_backend::{ObjectBackend, StoreCapabilities};
        use std::sync::{atomic::AtomicUsize, Barrier};
        struct Parallel {
            inner: Recording,
            active: AtomicUsize,
            peak: AtomicUsize,
            calls: AtomicUsize,
            hints: AtomicUsize,
            barrier: Barrier,
            workers: usize,
        }
        impl ObjectBackend for Parallel {
            fn capabilities(&self) -> StoreCapabilities {
                self.inner.capabilities()
            }
            fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
                self.inner.get(id)
            }
            fn exists(&self, id: ObjectId) -> Result<bool> {
                self.inner.exists(id)
            }
            fn max_concurrent_writes(&self) -> usize {
                self.hints.fetch_add(1, Ordering::SeqCst);
                usize::MAX
            }
            fn put_if_absent(&self, id: ObjectId, bytes: &[u8]) -> Result<()> {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(active, Ordering::SeqCst);
                if self.calls.fetch_add(1, Ordering::SeqCst) < self.workers {
                    self.barrier.wait();
                }
                let result = self.inner.put_if_absent(id, bytes);
                self.active.fetch_sub(1, Ordering::SeqCst);
                result
            }
        }
        let workers = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(8);
        let backend = Arc::new(Parallel {
            inner: Recording::default(),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            hints: AtomicUsize::new(0),
            barrier: Barrier::new(workers),
            workers,
        });
        let store = ObjectStore::with_backend(backend.clone()).unwrap();
        let mut publication = store.publisher();
        for n in 0..MAX_PUBLICATION_OBJECTS * 2 {
            let bytes = encode_object(BLOB, n.to_string().as_bytes()).unwrap();
            publication.push(object_id(&bytes), bytes).unwrap();
        }
        publication.finish().unwrap();
        assert_eq!(backend.active.load(Ordering::SeqCst), 0);
        assert_eq!(backend.peak.load(Ordering::SeqCst), workers);
        assert_eq!(backend.hints.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn publication_rejects_invalid_input_before_io_and_verifies_live_readback() {
        let backend = Arc::new(Recording::default());
        let store = ObjectStore::with_backend(backend.clone()).unwrap();
        let valid = encode_object(BLOB, b"valid").unwrap();
        let mut publication = store.publisher();
        publication.push(object_id(&valid), valid).unwrap();
        assert!(publication
            .push(ObjectId([0; 32]), b"wrong hash".to_vec())
            .is_err());
        let malformed = b"not an envelope".to_vec();
        assert!(publication.push(object_id(&malformed), malformed).is_err());
        drop(publication);
        assert_eq!(backend.puts.load(Ordering::SeqCst), 0);

        backend.corrupt.store(true, Ordering::SeqCst);
        let bytes = encode_object(BLOB, b"must read back exactly").unwrap();
        let mut publication = store.publisher();
        publication.push(object_id(&bytes), bytes).unwrap();
        assert!(publication.finish().is_err());
    }

    #[test]
    fn publication_uses_one_backend_batch_and_still_live_verifies_every_object() {
        use crate::storage_backend::{ObjectBackend, StoreCapabilities};
        #[derive(Default)]
        struct Batched {
            values: std::sync::Mutex<std::collections::HashMap<ObjectId, Vec<u8>>>,
            batches: std::sync::atomic::AtomicUsize,
            scalar_calls: std::sync::atomic::AtomicUsize,
            reads: std::sync::atomic::AtomicUsize,
            live_mode: std::sync::atomic::AtomicUsize,
            live_calls: std::sync::atomic::AtomicUsize,
        }
        impl ObjectBackend for Batched {
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
                self.reads.fetch_add(1, Ordering::SeqCst);
                self.values
                    .lock()
                    .unwrap()
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| Error::Io(std::io::ErrorKind::NotFound.into()))
            }
            fn get_batch_live(
                &self,
                ids: &[ObjectId],
                max_bytes: usize,
            ) -> Result<Option<Vec<Vec<u8>>>> {
                self.live_calls.fetch_add(1, Ordering::SeqCst);
                let mode = self.live_mode.load(Ordering::SeqCst);
                if mode == 0 {
                    return Ok(None);
                }
                if mode == 6 {
                    return Err(Error::Invalid("injected live batch read failure".into()));
                }
                let values = self.values.lock().unwrap();
                let mut bytes: Vec<_> = ids.iter().map(|id| values[id].clone()).collect();
                assert_eq!(bytes.iter().map(Vec::len).sum::<usize>(), max_bytes);
                match mode {
                    2 => {
                        bytes.pop();
                    }
                    3 => bytes.push(bytes[0].clone()),
                    4 => bytes.reverse(),
                    5 => bytes[0][15] ^= 1,
                    7 => bytes.last_mut().unwrap().push(0),
                    _ => {}
                }
                Ok(Some(bytes))
            }
            fn put_if_absent(&self, _: ObjectId, _: &[u8]) -> Result<()> {
                self.scalar_calls.fetch_add(1, Ordering::SeqCst);
                unreachable!("batch-capable provider must not receive scalar fallback")
            }
            fn put_batch_if_absent(&self, objects: &[(ObjectId, &[u8])]) -> Result<bool> {
                self.batches.fetch_add(1, Ordering::SeqCst);
                let mut values = self.values.lock().unwrap();
                for (id, bytes) in objects {
                    values.entry(*id).or_insert_with(|| bytes.to_vec());
                }
                Ok(true)
            }
            fn exists(&self, id: ObjectId) -> Result<bool> {
                Ok(self.values.lock().unwrap().contains_key(&id))
            }
        }

        for mode in 0..=7 {
            let backend = Arc::new(Batched::default());
            backend.live_mode.store(mode, Ordering::SeqCst);
            let store = ObjectStore::with_backend(backend.clone()).unwrap();
            let publish = || {
                let mut publication = store.publisher();
                for index in 0..64 {
                    let bytes =
                        encode_object(BLOB, format!("batch object {index}").as_bytes()).unwrap();
                    publication.push(object_id(&bytes), bytes).unwrap();
                }
                publication.finish()
            };
            assert_eq!(publish().is_ok(), mode <= 1);
            assert_eq!(backend.batches.load(Ordering::SeqCst), 1);
            assert_eq!(backend.live_calls.load(Ordering::SeqCst), 1);
            assert_eq!(backend.scalar_calls.load(Ordering::SeqCst), 0);
            assert_eq!(
                backend.reads.load(Ordering::SeqCst),
                if mode == 0 { 64 } else { 0 }
            );
            assert_eq!(backend.values.lock().unwrap().len(), 64);
            if mode > 1 {
                backend.live_mode.store(1, Ordering::SeqCst);
                publish().unwrap();
                assert_eq!(backend.values.lock().unwrap().len(), 64);
            }
        }
    }

    #[test]
    fn publication_reuses_one_bounded_write_session_across_windows() {
        use crate::storage_backend::{ObjectBackend, ObjectWriteSession, StoreCapabilities};
        #[derive(Default)]
        struct State {
            values: std::sync::Mutex<std::collections::HashMap<ObjectId, Vec<u8>>>,
            opens: std::sync::atomic::AtomicUsize,
            session_batches: std::sync::atomic::AtomicUsize,
            direct_batches: std::sync::atomic::AtomicUsize,
            live_reads: std::sync::atomic::AtomicUsize,
        }
        struct Session(std::sync::Arc<State>);
        impl ObjectWriteSession for Session {
            fn put_batch_if_absent(&mut self, objects: &[(ObjectId, &[u8])]) -> Result<bool> {
                self.0.session_batches.fetch_add(1, Ordering::SeqCst);
                let mut values = self.0.values.lock().unwrap();
                for (id, bytes) in objects {
                    values.entry(*id).or_insert_with(|| bytes.to_vec());
                }
                Ok(true)
            }
        }
        struct SessionBackend(std::sync::Arc<State>);
        impl ObjectBackend for SessionBackend {
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
                self.0
                    .values
                    .lock()
                    .unwrap()
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| Error::Io(std::io::ErrorKind::NotFound.into()))
            }
            fn get_batch_live(&self, ids: &[ObjectId], _: usize) -> Result<Option<Vec<Vec<u8>>>> {
                self.0.live_reads.fetch_add(1, Ordering::SeqCst);
                let values = self.0.values.lock().unwrap();
                ids.iter()
                    .map(|id| {
                        values
                            .get(id)
                            .cloned()
                            .ok_or_else(|| Error::Io(std::io::ErrorKind::NotFound.into()))
                    })
                    .collect::<Result<Vec<_>>>()
                    .map(Some)
            }
            fn put_if_absent(&self, _: ObjectId, _: &[u8]) -> Result<()> {
                unreachable!("session-capable backend must not use scalar publication")
            }
            fn put_batch_if_absent(&self, _: &[(ObjectId, &[u8])]) -> Result<bool> {
                self.0.direct_batches.fetch_add(1, Ordering::SeqCst);
                Ok(false)
            }
            fn open_write_session(&self) -> Result<Option<Box<dyn ObjectWriteSession>>> {
                self.0.opens.fetch_add(1, Ordering::SeqCst);
                Ok(Some(Box::new(Session(self.0.clone()))))
            }
            fn exists(&self, id: ObjectId) -> Result<bool> {
                Ok(self.0.values.lock().unwrap().contains_key(&id))
            }
        }

        let state = std::sync::Arc::new(State::default());
        let store =
            ObjectStore::with_backend(std::sync::Arc::new(SessionBackend(state.clone()))).unwrap();
        let mut publication = store.publisher();
        for index in 0..=MAX_PUBLICATION_OBJECTS {
            let bytes = encode_object(BLOB, format!("session object {index}").as_bytes()).unwrap();
            publication.push(object_id(&bytes), bytes).unwrap();
        }
        publication.finish().unwrap();
        assert_eq!(state.opens.load(Ordering::SeqCst), 1);
        assert_eq!(state.session_batches.load(Ordering::SeqCst), 2);
        assert_eq!(state.direct_batches.load(Ordering::SeqCst), 0);
        assert_eq!(state.live_reads.load(Ordering::SeqCst), 2);
        assert_eq!(
            state.values.lock().unwrap().len(),
            MAX_PUBLICATION_OBJECTS + 1
        );
    }

    #[test]
    fn closure_live_reads_validate_bytes_and_preserve_all_copy_fallback() {
        use crate::storage_backend::{ObjectBackend, StoreCapabilities};
        #[derive(Default)]
        struct Live {
            inner: Recording,
            batch: std::sync::atomic::AtomicBool,
            missing_copy: std::sync::atomic::AtomicBool,
            scalar_reads: std::sync::atomic::AtomicUsize,
            exists: std::sync::atomic::AtomicUsize,
        }
        impl ObjectBackend for Live {
            fn capabilities(&self) -> StoreCapabilities {
                self.inner.capabilities()
            }
            fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
                self.scalar_reads.fetch_add(1, Ordering::SeqCst);
                self.inner.get(id)
            }
            fn put_if_absent(&self, id: ObjectId, bytes: &[u8]) -> Result<()> {
                self.inner.put_if_absent(id, bytes)
            }
            fn exists(&self, id: ObjectId) -> Result<bool> {
                self.exists.fetch_add(1, Ordering::SeqCst);
                Ok(!self.missing_copy.load(Ordering::SeqCst) && self.inner.exists(id)?)
            }
            fn get_batch_live(&self, ids: &[ObjectId], _: usize) -> Result<Option<Vec<Vec<u8>>>> {
                if !self.batch.load(Ordering::SeqCst) {
                    return Ok(None);
                }
                ids.iter()
                    .map(|id| self.inner.get(*id))
                    .collect::<Result<Vec<_>>>()
                    .map(Some)
            }
        }
        let backend = Arc::new(Live::default());
        let store = ObjectStore::with_backend(backend.clone()).unwrap();
        let id = store.put(BLOB, b"live typed closure").unwrap();
        backend.scalar_reads.store(0, Ordering::SeqCst);
        store.verify_external_closure(&[(id, BLOB)]).unwrap();
        assert_eq!(backend.exists.load(Ordering::SeqCst), 1);
        assert_eq!(backend.scalar_reads.load(Ordering::SeqCst), 1);
        backend.missing_copy.store(true, Ordering::SeqCst);
        assert!(store.verify_external_closure(&[(id, BLOB)]).is_err());
        assert_eq!(backend.scalar_reads.load(Ordering::SeqCst), 1);
        backend.missing_copy.store(false, Ordering::SeqCst);
        backend.batch.store(true, Ordering::SeqCst);
        store.verify_external_closure(&[(id, BLOB)]).unwrap();
        assert_eq!(backend.exists.load(Ordering::SeqCst), 2);
        assert_eq!(backend.scalar_reads.load(Ordering::SeqCst), 1);
        assert!(store
            .verify_external_closure(&[(id, crate::format::CHANGESET)])
            .is_err());
        backend.inner.corrupt.store(true, Ordering::SeqCst);
        assert!(store.verify_external_closure(&[(id, BLOB)]).is_err());
        backend.inner.corrupt.store(false, Ordering::SeqCst);
        let malformed = b"hash-matching malformed envelope".to_vec();
        let bad = object_id(&malformed);
        backend.inner.values.lock().unwrap().insert(bad, malformed);
        assert!(store.verify_external_closure(&[(bad, BLOB)]).is_err());
    }

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
    fn read_session_facade_does_not_trust_provider_hashes() {
        use crate::storage_backend::{ObjectBackend, ObjectReadSession, StoreCapabilities};
        struct Untrusted;
        impl ObjectReadSession for Untrusted {
            fn get(&self, _: ObjectId) -> Result<Vec<u8>> {
                encode_object(BLOB, b"wrong")
            }
        }
        impl ObjectBackend for Untrusted {
            fn capabilities(&self) -> StoreCapabilities {
                StoreCapabilities {
                    durable_writes: true,
                    conditional_create: true,
                    streaming_reads: false,
                    streaming_writes: false,
                    list_objects: false,
                }
            }
            fn get(&self, _: ObjectId) -> Result<Vec<u8>> {
                unreachable!()
            }
            fn put_if_absent(&self, _: ObjectId, _: &[u8]) -> Result<()> {
                unreachable!()
            }
            fn exists(&self, _: ObjectId) -> Result<bool> {
                unreachable!()
            }
            fn open_read_session(&self) -> Result<Option<Arc<dyn ObjectReadSession>>> {
                Ok(Some(Arc::new(Untrusted)))
            }
        }
        let store = ObjectStore::with_backend(Arc::new(Untrusted)).unwrap();
        let id = object_id(&encode_object(BLOB, b"expected").unwrap());
        assert!(store.read_session().unwrap().get(id).is_err());
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
        let ids: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().unwrap().unwrap())
            .collect();
        assert!(ids.iter().all(|id| *id == ids[0]));
        assert_eq!(store.ids().unwrap(), vec![ids[0]]);
        assert_eq!(
            store.get(ids[0]).unwrap(),
            encode_object(BLOB, b"same payload").unwrap()
        );
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
        file.set_len((crate::format::MAX_OBJECT_SIZE + 16) as u64)
            .unwrap();
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
        assert_eq!(
            fs::read(&alias).unwrap(),
            encode_object(BLOB, b"protected").unwrap()
        );
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
