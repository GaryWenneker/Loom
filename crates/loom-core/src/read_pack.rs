//! Disposable immutable read acceleration. Never object existence authority.
use crate::format::{decode_object, object_id, BLOB, MAX_OBJECT_SIZE, TREE};
use crate::storage_backend::ObjectReadSession;
use crate::store::ObjectStore;
use crate::{Error, NativeTree, ObjectId, Result, TreeEntryKind};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
#[cfg(not(unix))]
use std::sync::Mutex;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

const MAGIC: &[u8; 8] = b"LOOMRDP1";
const FOOTER: &[u8; 8] = b"LOOMIDX1";
const RECORD: usize = 48;
const FOOTER_SIZE: usize = 56;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
static NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
pub struct ReadPackStats {
    pub objects: usize,
    pub bytes: u64,
}
pub(crate) struct ReadPack {
    #[cfg(unix)]
    file: File,
    #[cfg(not(unix))]
    file: Mutex<File>,
    index: Vec<u8>,
}

fn invalid() -> Error {
    Error::Invalid("invalid read pack; rebuild it from live objects".into())
}

fn open_file(path: &Path) -> Result<File> {
    #[cfg(unix)]
    let file = File::from(
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    );
    #[cfg(not(unix))]
    let file = {
        if fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(invalid());
        }
        OpenOptions::new().read(true).open(path)?
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for attempt in 0..100 {
            if file.metadata()?.nlink() == 1 {
                break;
            }
            if attempt == 99 {
                return Err(invalid());
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    Ok(file)
}

impl ReadPack {
    pub(crate) fn index_bytes(&self) -> usize {
        self.index.capacity()
    }
    pub(crate) fn objects(&self) -> usize {
        self.index.len() / RECORD
    }
    #[cfg(test)]
    pub(crate) fn open(path: &Path) -> Result<Self> {
        Self::open_with_index_reservation(path, |_| Ok(()))
    }
    pub(crate) fn open_with_index_reservation(
        path: &Path,
        reserve: impl FnOnce(usize) -> Result<()>,
    ) -> Result<Self> {
        let mut file = open_file(path)?;
        let size = file.metadata()?.len();
        if size > MAX_BYTES || size < (MAGIC.len() + FOOTER_SIZE) as u64 {
            return Err(invalid());
        }
        let mut magic = [0; 8];
        file.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(invalid());
        }
        file.seek(SeekFrom::End(-(FOOTER_SIZE as i64)))?;
        let mut footer = [0; FOOTER_SIZE];
        file.read_exact(&mut footer)?;
        if &footer[..8] != FOOTER {
            return Err(invalid());
        }
        let start = u64::from_le_bytes(footer[8..16].try_into().expect("eight bytes"));
        let count = u64::from_le_bytes(footer[16..24].try_into().expect("eight bytes"));
        if count == 0
            || count > MAX_ENTRIES as u64
            || start < 8
            || start.checked_add(count * RECORD as u64 + FOOTER_SIZE as u64) != Some(size)
        {
            return Err(invalid());
        }
        let index_bytes = count as usize * RECORD;
        reserve(index_bytes)?;
        let mut bytes = vec![0; index_bytes];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut bytes)?;
        if blake3::hash(&bytes).as_bytes() != &footer[24..] {
            return Err(invalid());
        }
        let mut previous = None;
        let mut next = 8;
        for record in bytes.chunks_exact(RECORD) {
            let id = &record[..32];
            let offset = u64::from_le_bytes(record[32..40].try_into().expect("eight bytes"));
            let len = u64::from_le_bytes(record[40..48].try_into().expect("eight bytes"));
            if previous.is_some_and(|previous| previous >= id)
                || offset != next
                || !(15..=(MAX_OBJECT_SIZE + 15) as u64).contains(&len)
                || offset.checked_add(len).is_none_or(|end| end > start)
            {
                return Err(invalid());
            }
            next = offset + len;
            previous = Some(id);
        }
        if next != start {
            return Err(invalid());
        }
        Ok(Self {
            #[cfg(unix)]
            file,
            #[cfg(not(unix))]
            file: Mutex::new(file),
            index: bytes,
        })
    }
    fn entry(&self, id: ObjectId) -> Option<(u64, usize)> {
        let (mut low, mut high) = (0, self.objects());
        while low < high {
            let middle = low + (high - low) / 2;
            let record = &self.index[middle * RECORD..(middle + 1) * RECORD];
            match record[..32].cmp(&id.0) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => {
                    return Some((
                        u64::from_le_bytes(record[32..40].try_into().expect("validated offset")),
                        u64::from_le_bytes(record[40..48].try_into().expect("validated length"))
                            as usize,
                    ))
                }
            }
        }
        None
    }
    fn get(&self, id: ObjectId) -> Result<Option<Vec<u8>>> {
        let Some((offset, length)) = self.entry(id) else {
            return Ok(None);
        };
        let mut bytes = vec![0; length];
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.file.read_exact_at(&mut bytes, offset)?;
        }
        #[cfg(not(unix))]
        {
            let mut file = self.file.lock().map_err(|_| invalid())?;
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut bytes)?;
        }
        // The index checksum is not object authentication. Verify every read.
        if object_id(&bytes) != id {
            return Err(invalid());
        }
        decode_object(&bytes)?;
        Ok(Some(bytes))
    }
}

pub(crate) struct PackedSession {
    pub pack: ReadPack,
    pub live: Arc<dyn ObjectReadSession>,
}
impl ObjectReadSession for PackedSession {
    fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
        match self.pack.get(id)? {
            Some(bytes) => Ok(bytes),
            None => self.live.get(id),
        }
    }
}

pub(crate) fn write(
    store: &ObjectStore,
    root: ObjectId,
    destination: &Path,
) -> Result<ReadPackStats> {
    // Capture a bounded immutable tree closure, without scanning unrelated history.
    let mut kinds = BTreeMap::from([(root, TREE)]);
    let mut depths = BTreeMap::new();
    let mut pending = vec![(root, 0)];
    let mut visits = 0;
    while let Some((id, depth)) = pending.pop() {
        visits += 1;
        if depth > 256 || visits > MAX_ENTRIES {
            return Err(invalid());
        }
        if depths.get(&id).is_some_and(|old| *old >= depth) {
            continue;
        }
        depths.insert(id, depth);
        let tree = NativeTree::decode(&store.get(id)?)?;
        if depth > 0 && tree.entries.is_empty() {
            return Err(invalid());
        }
        for entry in tree.entries {
            let kind = if entry.kind == TreeEntryKind::File {
                BLOB
            } else {
                TREE
            };
            if kinds
                .insert(entry.object, kind)
                .is_some_and(|old| old != kind)
            {
                return Err(invalid());
            }
            if kinds.len() > MAX_ENTRIES {
                return Err(invalid());
            }
            if kind == TREE {
                if pending.len() >= MAX_ENTRIES {
                    return Err(invalid());
                }
                pending.push((entry.object, depth + 1));
            }
        }
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid());
    }
    let temporary = parent.join(format!(
        ".loom-read-pack-{}-{}.tmp",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(MAGIC)?;
        let mut index = Vec::with_capacity(kinds.len() * RECORD);
        let mut offset = 8u64;
        for (id, kind) in &kinds {
            let bytes = store.get(*id)?;
            if decode_object(&bytes)?.0 != *kind {
                return Err(invalid());
            }
            let end = offset + bytes.len() as u64;
            if end + kinds.len() as u64 * RECORD as u64 + FOOTER_SIZE as u64 > MAX_BYTES {
                return Err(invalid());
            }
            file.write_all(&bytes)?;
            index.extend_from_slice(&id.0);
            index.extend_from_slice(&offset.to_le_bytes());
            index.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            offset = end;
        }
        file.write_all(&index)?;
        file.write_all(FOOTER)?;
        file.write_all(&offset.to_le_bytes())?;
        file.write_all(&(kinds.len() as u64).to_le_bytes())?;
        file.write_all(blake3::hash(&index).as_bytes())?;
        file.sync_all()?;
        drop(file);
        // Atomic create-if-absent; interrupted builds never replace a pack.
        fs::hard_link(&temporary, destination)?;
        fs::remove_file(&temporary)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(ReadPackStats {
            objects: kinds.len(),
            bytes: offset + index.len() as u64 + FOOTER_SIZE as u64,
        })
    })();
    if temporary.exists() {
        fs::remove_file(&temporary)?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{format::encode_object, TreeEntry};
    fn fixture() -> (tempfile::TempDir, ObjectStore, ObjectId, ObjectId) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("objects")).unwrap();
        let store = ObjectStore::new(dir.path());
        let blob = store.put(BLOB, b"binary\0\xff\n").unwrap();
        let empty = store.put(BLOB, b"").unwrap();
        let tree = NativeTree {
            entries: vec![
                TreeEntry {
                    name: "a".into(),
                    kind: TreeEntryKind::File,
                    executable: false,
                    object: blob,
                },
                TreeEntry {
                    name: "b".into(),
                    kind: TreeEntryKind::File,
                    executable: true,
                    object: blob,
                },
                TreeEntry {
                    name: "empty".into(),
                    kind: TreeEntryKind::File,
                    executable: false,
                    object: empty,
                },
            ],
        }
        .encode()
        .unwrap();
        let root = store.put(TREE, decode_object(&tree).unwrap().1).unwrap();
        (dir, store, root, blob)
    }
    #[test]
    fn roundtrip_deduplicates_and_preserves_live_objects() {
        let (dir, store, root, blob) = fixture();
        let path = dir.path().join("read.pack");
        let ids = store.ids().unwrap();
        let stats = write(&store, root, &path).unwrap();
        assert_eq!(stats.objects, 3);
        assert_eq!(stats.bytes, fs::metadata(&path).unwrap().len());
        let pack = ReadPack::open(&path).unwrap();
        for id in &ids {
            assert_eq!(pack.get(*id).unwrap().unwrap(), store.get(*id).unwrap());
        }
        assert_eq!(pack.get(ObjectId([0; 32])).unwrap(), None);
        assert_eq!(store.ids().unwrap(), ids);
        let encoded = encode_object(BLOB, b"new live object").unwrap();
        let newer = store.put(BLOB, b"new live object").unwrap();
        let session = PackedSession {
            pack,
            live: store.read_session().unwrap(),
        };
        assert_eq!(session.get(newer).unwrap(), encoded);
        // Cached bytes cannot certify live existence or successful publication.
        fs::remove_file(store.path_for_test(blob)).unwrap();
        assert!(store.get(blob).is_err());
        assert_eq!(
            session.get(blob).unwrap(),
            encode_object(BLOB, b"binary\0\xff\n").unwrap()
        );
    }
    fn rehash(bytes: &mut [u8]) {
        let footer = bytes.len() - FOOTER_SIZE;
        let start = u64::from_le_bytes(bytes[footer + 8..footer + 16].try_into().unwrap()) as usize;
        let hash = blake3::hash(&bytes[start..footer]);
        bytes[footer + 24..].copy_from_slice(hash.as_bytes());
    }
    #[test]
    fn malformed_and_adversarial_indexes_are_bounded_and_rejected() {
        let (dir, store, root, _) = fixture();
        let path = dir.path().join("read.pack");
        write(&store, root, &path).unwrap();
        let good = fs::read(&path).unwrap();
        let footer = good.len() - FOOTER_SIZE;
        let start = u64::from_le_bytes(good[footer + 8..footer + 16].try_into().unwrap()) as usize;
        let bad = dir.path().join("bad.pack");
        for len in [0, 8, 55, good.len() - 1] {
            fs::write(&bad, &good[..len]).unwrap();
            assert!(ReadPack::open(&bad).is_err());
        }
        for mode in 0..8 {
            let mut bytes = good.clone();
            match mode {
                0 => bytes[0] ^= 1,
                1 => bytes[footer] ^= 1,
                2 => bytes[footer + 16..footer + 24].copy_from_slice(&u64::MAX.to_le_bytes()),
                3 => bytes[footer + 8..footer + 16].copy_from_slice(&u64::MAX.to_le_bytes()),
                4 => bytes[start] ^= 1, // checksum mismatch
                5 => {
                    bytes[start + 32..start + 40].copy_from_slice(&u64::MAX.to_le_bytes());
                    rehash(&mut bytes);
                }
                6 => {
                    bytes[start + 40..start + 48].copy_from_slice(&u64::MAX.to_le_bytes());
                    rehash(&mut bytes);
                }
                _ => {
                    let id = bytes[start..start + 32].to_vec();
                    bytes[start + 48..start + 80].copy_from_slice(&id);
                    rehash(&mut bytes);
                }
            }
            fs::write(&bad, bytes).unwrap();
            assert!(ReadPack::open(&bad).is_err(), "case {mode}");
        }
    }
    #[test]
    fn valid_index_never_authenticates_corrupt_object_bytes() {
        let (dir, store, root, blob) = fixture();
        let path = dir.path().join("read.pack");
        write(&store, root, &path).unwrap();
        let pack = ReadPack::open(&path).unwrap();
        let (offset, _) = pack.entry(blob).unwrap();
        let mut file = OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(offset + 15)).unwrap();
        file.write_all(b"X").unwrap();
        // Even the retained session must verify data after the pack is opened.
        assert!(pack.get(blob).is_err());
        assert!(PackedSession {
            pack,
            live: store.read_session().unwrap()
        }
        .get(blob)
        .is_err());
        assert!(store.get(blob).is_ok());
    }
    #[test]
    fn failed_build_and_existing_destination_preserve_everything() {
        let (dir, store, root, blob) = fixture();
        let path = dir.path().join("read.pack");
        fs::write(&path, b"user content").unwrap();
        assert!(write(&store, root, &path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"user content");
        fs::remove_file(&path).unwrap();
        fs::remove_file(store.path_for_test(blob)).unwrap();
        assert!(write(&store, root, &path).is_err());
        assert!(!path.exists());
        assert!(fs::read_dir(dir.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }
    #[test]
    fn competing_builders_publish_one_complete_file() {
        let (dir, store, root, _) = fixture();
        let path = dir.path().join("read.pack");
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        write(&store, root, &path)
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|job| job.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let pack = ReadPack::open(&path).unwrap();
        assert_eq!(pack.get(root).unwrap().unwrap(), store.get(root).unwrap());
        assert!(fs::read_dir(dir.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_hardlinks_fifos_and_large_pack_files_are_rejected() {
        let (dir, store, root, _) = fixture();
        let path = dir.path().join("read.pack");
        write(&store, root, &path).unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert!(ReadPack::open(&alias).is_err());
        fs::remove_file(&alias).unwrap();
        fs::hard_link(&path, &alias).unwrap();
        assert!(ReadPack::open(&path).is_err());
        fs::remove_file(&alias).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(&alias)
            .status()
            .unwrap()
            .success());
        assert!(ReadPack::open(&alias).is_err());
        fs::remove_file(&alias).unwrap();
        File::create(&alias)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(ReadPack::open(&alias).is_err());
    }
}
