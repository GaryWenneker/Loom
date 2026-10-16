//! Revision-pinned queries with a bounded cache of verified immutable content.
//! Cache entries are never consulted for publication, recovery or fsck.
use crate::format::{decode_object, validate_path, BLOB};
use crate::store::ObjectStore;
use crate::{Error, NativeTree, ObjectId, Result, TreeEntryKind};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

pub const DEFAULT_SNAPSHOT_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_CACHE_BYTES: usize = 256 * 1024 * 1024;
const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_FRONTIER_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default)]
pub struct SnapshotStats {
    pub tree_reads: u64,
    pub blob_reads: u64,
    pub cache_hits: u64,
    pub cache_bytes: usize,
    pub cache_entries: usize,
}

#[derive(Debug, Clone)]
pub struct SnapshotFiles {
    pub paths: Vec<String>,
    pub next_cursor: Option<String>,
}

enum Cached {
    Tree(Arc<NativeTree>),
    Blob(Arc<[u8]>),
}
impl Clone for Cached {
    fn clone(&self) -> Self {
        match self {
            Self::Tree(value) => Self::Tree(value.clone()),
            Self::Blob(value) => Self::Blob(value.clone()),
        }
    }
}
struct Cache {
    entries: HashMap<ObjectId, (Cached, usize)>,
    order: VecDeque<ObjectId>,
    budget: usize,
    stats: SnapshotStats,
}
impl Cache {
    fn get(&mut self, id: ObjectId) -> Option<Cached> {
        let value = self.entries.get(&id).map(|(value, _)| value.clone());
        if value.is_some() {
            self.stats.cache_hits += 1;
        }
        value
    }
    fn insert(&mut self, id: ObjectId, value: Cached, bytes: usize) {
        // Include an allowance for Arc, map and queue bookkeeping per object.
        let weight = bytes.saturating_add(128);
        if weight > self.budget || self.entries.contains_key(&id) {
            return;
        }
        while self.stats.cache_bytes.saturating_add(weight) > self.budget {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some((_, size)) = self.entries.remove(&oldest) {
                self.stats.cache_bytes -= size;
            }
        }
        self.stats.cache_bytes += weight;
        self.entries.insert(id, (value, weight));
        self.order.push_back(id);
        self.stats.cache_entries = self.entries.len();
    }
}

/// Immutable tree-root view. Branch movement cannot change its answers.
/// Verified bytes/decoded trees may be retained until eviction. A later source
/// deletion/corruption does not invalidate already verified canonical content;
/// new sessions, fsck and all publication checks still read the live provider.
/// The cache budget accounts retained content and estimated bookkeeping, not
/// total process RSS or Arc values retained by callers.
pub struct Snapshot {
    root: ObjectId,
    reader: Arc<dyn crate::storage_backend::ObjectReadSession>,
    cache: Mutex<Cache>,
}
impl Snapshot {
    pub(crate) fn new(root: ObjectId, store: ObjectStore, budget: usize) -> Result<Self> {
        if budget > MAX_CACHE_BYTES {
            return Err(Error::Invalid(
                "snapshot cache exceeds 256 MiB limit".into(),
            ));
        }
        let snapshot = Self {
            root,
            reader: store.read_session()?,
            cache: Mutex::new(Cache {
                entries: HashMap::new(),
                order: VecDeque::new(),
                budget,
                stats: SnapshotStats::default(),
            }),
        };
        snapshot.tree(root, 0)?;
        Ok(snapshot)
    }
    fn cache(&self) -> Result<std::sync::MutexGuard<'_, Cache>> {
        self.cache
            .lock()
            .map_err(|_| Error::Invalid("snapshot cache lock poisoned".into()))
    }
    pub fn root(&self) -> ObjectId {
        self.root
    }
    pub fn stats(&self) -> Result<SnapshotStats> {
        Ok(self.cache()?.stats)
    }

    fn tree(&self, id: ObjectId, depth: usize) -> Result<Arc<NativeTree>> {
        if depth > 256 {
            return Err(Error::Invalid("snapshot nesting limit reached".into()));
        }
        if let Some(value) = self.cache()?.get(id) {
            return match value {
                Cached::Tree(tree) if depth == 0 || !tree.entries.is_empty() => Ok(tree),
                _ => Err(Error::Invalid(
                    "noncanonical snapshot tree reference".into(),
                )),
            };
        }
        let bytes = self.reader.get(id)?;
        let tree = Arc::new(NativeTree::decode(&bytes)?);
        if depth > 0 && tree.entries.is_empty() {
            return Err(Error::Invalid(
                "noncanonical empty snapshot directory".into(),
            ));
        }
        let weight = std::mem::size_of::<NativeTree>()
            + tree.entries.capacity() * std::mem::size_of::<crate::TreeEntry>()
            + tree
                .entries
                .iter()
                .map(|entry| entry.name.capacity())
                .sum::<usize>();
        let mut cache = self.cache()?;
        cache.stats.tree_reads += 1;
        cache.insert(id, Cached::Tree(tree.clone()), weight);
        Ok(tree)
    }
    fn blob(&self, id: ObjectId) -> Result<Arc<[u8]>> {
        if let Some(value) = self.cache()?.get(id) {
            return match value {
                Cached::Blob(bytes) => Ok(bytes),
                _ => Err(Error::Invalid("snapshot file references a tree".into())),
            };
        }
        let bytes = self.reader.get(id)?;
        let (kind, payload) = decode_object(&bytes)?;
        if kind != BLOB {
            return Err(Error::Invalid("snapshot file references a non-blob".into()));
        }
        let content: Arc<[u8]> = Arc::from(payload);
        let mut cache = self.cache()?;
        cache.stats.blob_reads += 1;
        cache.insert(id, Cached::Blob(content.clone()), content.len());
        Ok(content)
    }
    fn resolve(&self, path: &str) -> Result<Option<(ObjectId, TreeEntryKind, usize)>> {
        validate_path(path)?;
        if path.len() > MAX_PAGE_BYTES {
            return Err(Error::Invalid(
                "snapshot path exceeds query size limit".into(),
            ));
        }
        let count = path.split('/').count();
        if count > 256 {
            return Err(Error::Invalid("snapshot path nesting limit reached".into()));
        }
        let mut current = self.root;
        for (depth, part) in path.split('/').enumerate() {
            let tree = self.tree(current, depth)?;
            let Ok(index) = tree.entries.binary_search_by(|e| e.name.as_str().cmp(part)) else {
                return Ok(None);
            };
            let entry = &tree.entries[index];
            if depth + 1 == count {
                return Ok(Some((entry.object, entry.kind, depth + 1)));
            }
            if entry.kind != TreeEntryKind::Directory {
                return Ok(None);
            }
            current = entry.object;
        }
        Ok(None)
    }
    pub fn read_file(&self, path: &str) -> Result<Option<Arc<[u8]>>> {
        match self.resolve(path)? {
            Some((id, TreeEntryKind::File, _)) => self.blob(id).map(Some),
            _ => Ok(None),
        }
    }
    /// Lexically ordered, bounded page. Prefix matches a complete file or
    /// directory component; the cursor is the last emitted full path.
    pub fn files(
        &self,
        prefix: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<SnapshotFiles> {
        if !(1..=100_000).contains(&limit) {
            return Err(Error::Invalid(
                "snapshot page size must be 1..=100000".into(),
            ));
        }
        if let Some(path) = after {
            validate_path(path)?;
            if path.len() > MAX_PAGE_BYTES {
                return Err(Error::Invalid(
                    "snapshot cursor exceeds query size limit".into(),
                ));
            }
        }
        let start = match prefix {
            Some(path) => self
                .resolve(path)?
                .map(|(id, kind, depth)| (path.to_owned(), id, kind, depth)),
            None => Some((String::new(), self.root, TreeEntryKind::Directory, 0)),
        };
        let mut pending = BTreeMap::new();
        if let Some((path, id, kind, depth)) = start {
            let key = if kind == TreeEntryKind::Directory && !path.is_empty() {
                format!("{path}/")
            } else {
                path
            };
            pending.insert(key, (id, kind, depth));
        }
        let mut paths = Vec::new();
        // Bound queued paths as well as returned paths. A compact shared DAG
        // can otherwise expand into a much larger enumeration frontier.
        let mut frontier_bytes = pending.keys().map(|p| p.len() + 128).sum::<usize>();
        let mut bytes = 0usize;
        let mut visited = 0usize;
        let mut more = false;
        while let Some((path, (id, kind, depth))) = pending.pop_first() {
            frontier_bytes -= path.len() + 128;
            visited += 1;
            if visited > 1_000_000 {
                return Err(Error::Invalid(
                    "snapshot listing traversal limit reached".into(),
                ));
            }
            if kind == TreeEntryKind::Directory {
                for entry in &self.tree(id, depth)?.entries {
                    let child = format!(
                        "{path}{}{}",
                        entry.name,
                        if entry.kind == TreeEntryKind::Directory {
                            "/"
                        } else {
                            ""
                        }
                    );
                    if child.len() > MAX_PAGE_BYTES {
                        return Err(Error::Invalid(
                            "snapshot listing path exceeds query size limit".into(),
                        ));
                    }
                    if depth >= 256 {
                        return Err(Error::Invalid(
                            "snapshot listing nesting limit reached".into(),
                        ));
                    }
                    let weight = child.len().saturating_add(128);
                    if frontier_bytes.saturating_add(weight) > MAX_FRONTIER_BYTES {
                        return Err(Error::Invalid(
                            "snapshot listing frontier exceeds 64 MiB limit; use a narrower prefix"
                                .into(),
                        ));
                    }
                    frontier_bytes += weight;
                    pending.insert(child, (entry.object, entry.kind, depth + 1));
                }
            } else if after.is_none_or(|cursor| path.as_str() > cursor) {
                if paths.len() >= limit || bytes.saturating_add(path.len()) > MAX_PAGE_BYTES {
                    more = true;
                    break;
                }
                bytes += path.len();
                paths.push(path);
            }
        }
        let next_cursor = if more { paths.last().cloned() } else { None };
        Ok(SnapshotFiles { paths, next_cursor })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_and_reading_enforce_the_same_path_depth_boundary() {
        let dir = tempfile::tempdir().unwrap();
        crate::Repository::init(dir.path()).unwrap();
        let store = ObjectStore::new(&dir.path().join(".loom"));
        let blob = store.put(BLOB, b"deep").unwrap();
        let write_tree = |name: &str, object, kind| {
            let encoded = NativeTree {
                entries: vec![crate::TreeEntry {
                    name: name.into(),
                    object,
                    kind,
                    executable: false,
                }],
            }
            .encode()
            .unwrap();
            let (kind, payload) = decode_object(&encoded).unwrap();
            store.put(kind, payload).unwrap()
        };
        let mut root = write_tree("z", blob, TreeEntryKind::File);
        for depth in 1..=256 {
            root = write_tree("a", root, TreeEntryKind::Directory);
            if depth == 255 {
                let snapshot =
                    Snapshot::new(root, store.clone(), DEFAULT_SNAPSHOT_CACHE_BYTES).unwrap();
                let path = format!("{}z", "a/".repeat(depth));
                assert_eq!(
                    snapshot.files(None, None, 1).unwrap().paths.as_slice(),
                    std::slice::from_ref(&path)
                );
                assert_eq!(&*snapshot.read_file(&path).unwrap().unwrap(), b"deep");
            }
        }
        let snapshot = Snapshot::new(root, store, DEFAULT_SNAPSHOT_CACHE_BYTES).unwrap();
        assert!(snapshot
            .files(None, None, 1)
            .unwrap_err()
            .to_string()
            .contains("nesting"));
        assert!(snapshot
            .read_file(&format!("{}z", "a/".repeat(256)))
            .is_err());
    }
}
