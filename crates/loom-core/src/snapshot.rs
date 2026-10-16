//! Revision-pinned queries with a bounded cache of verified immutable content.
//! Cache entries are never consulted for publication, recovery or fsck.
use crate::format::{decode_object, validate_path, BLOB};
use crate::store::ObjectStore;
use crate::tree_index::IndexedTree;
use crate::{Error, ObjectId, Result, TreeEntryKind};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

fn parallel_map<T: Sync, R: Send>(
    work: &[T],
    workers: usize,
    load: &(impl Fn(&T) -> Result<R> + Sync),
) -> Result<Vec<R>> {
    if workers <= 1 || work.len() < 32 {
        return work.iter().map(load).collect();
    }
    std::thread::scope(|scope| {
        let tasks: Vec<_> = work
            .chunks(work.len().div_ceil(workers))
            .map(|chunk| scope.spawn(move || chunk.iter().map(load).collect::<Result<Vec<_>>>()))
            .collect();
        let mut values = Vec::with_capacity(work.len());
        let mut error = None;
        // Join every worker, including failures; no read survives the call.
        for task in tasks {
            match task.join() {
                Ok(Ok(items)) => values.extend(items),
                Ok(Err(cause)) => {
                    if error.is_none() {
                        error = Some(cause);
                    }
                }
                Err(_) => {
                    if error.is_none() {
                        error = Some(Error::Invalid("snapshot read worker panicked".into()));
                    }
                }
            }
        }
        match error {
            Some(error) => Err(error),
            None => Ok(values),
        }
    })
}

pub const DEFAULT_SNAPSHOT_CACHE_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_BULK_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_BULK_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_BULK_PATHS: usize = 4096;
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
    pub path_cache_hits: u64,
    pub path_cache_entries: usize,
    /// Retained read-pack index allocation, separate from cache_bytes/budget.
    pub read_pack_index_bytes: usize,
    pub read_pack_objects: usize,
}

#[derive(Debug, Clone)]
pub struct SnapshotFiles {
    pub paths: Vec<String>,
    pub next_cursor: Option<String>,
}

enum Cached {
    Tree(Arc<IndexedTree>),
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
enum CacheKey {
    Object(ObjectId),
    Path(String),
}
struct PathEntry {
    content: Option<Arc<[u8]>>,
    weight: usize,
}
struct Cache {
    entries: HashMap<ObjectId, (Cached, usize)>,
    paths: HashMap<String, PathEntry>,
    order: VecDeque<CacheKey>,
    budget: usize,
    stats: SnapshotStats,
}
impl Cache {
    fn get_path(&mut self, path: &str) -> Option<Option<Arc<[u8]>>> {
        let value = self.paths.get(path).map(|entry| entry.content.clone());
        if value.is_some() {
            self.stats.cache_hits += 1;
            self.stats.path_cache_hits += 1;
        }
        value
    }
    fn make_room(&mut self, weight: usize) {
        while self.stats.cache_bytes.saturating_add(weight) > self.budget {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            let removed = match oldest {
                CacheKey::Object(id) => self.entries.remove(&id).map(|(_, size)| size),
                CacheKey::Path(path) => self.paths.remove(&path).map(|entry| entry.weight),
            };
            if let Some(size) = removed {
                self.stats.cache_bytes -= size;
            }
        }
    }
    fn update_counts(&mut self) {
        self.stats.path_cache_entries = self.paths.len();
        self.stats.cache_entries = self.entries.len() + self.paths.len();
    }
    fn insert_path(&mut self, path: &str, value: Option<Arc<[u8]>>) {
        // Count the payload for every retained path conservatively, even when
        // an object-cache entry or another path shares the same Arc. Account
        // both owned path strings (map and FIFO), plus bookkeeping.
        let weight = value
            .as_ref()
            .map_or(0, |bytes| bytes.len())
            .saturating_add(path.len().saturating_mul(2))
            .saturating_add(256);
        if weight > self.budget || self.paths.contains_key(path) {
            return;
        }
        self.make_room(weight);
        self.stats.cache_bytes += weight;
        self.paths.insert(
            path.to_owned(),
            PathEntry {
                content: value,
                weight,
            },
        );
        self.order.push_back(CacheKey::Path(path.to_owned()));
        self.update_counts();
    }
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
        self.make_room(weight);
        self.stats.cache_bytes += weight;
        self.entries.insert(id, (value, weight));
        self.order.push_back(CacheKey::Object(id));
        self.update_counts();
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
        Self::with_read_pack(root, store, budget, None)
    }
    pub(crate) fn with_read_pack(
        root: ObjectId,
        store: ObjectStore,
        budget: usize,
        pack: Option<&std::path::Path>,
    ) -> Result<Self> {
        if budget > MAX_CACHE_BYTES {
            return Err(Error::Invalid(
                "snapshot cache exceeds 256 MiB limit".into(),
            ));
        }
        let live = store.read_session()?;
        let mut stats = SnapshotStats::default();
        let reader: Arc<dyn crate::storage_backend::ObjectReadSession> = match pack {
            Some(path) => {
                let pack = crate::read_pack::ReadPack::open(path)?;
                stats.read_pack_index_bytes = pack.index_bytes();
                stats.read_pack_objects = pack.objects();
                Arc::new(crate::read_pack::PackedSession { pack, live })
            }
            None => live,
        };
        let snapshot = Self {
            root,
            reader,
            cache: Mutex::new(Cache {
                entries: HashMap::new(),
                paths: HashMap::new(),
                order: VecDeque::new(),
                budget,
                stats,
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

    fn tree(&self, id: ObjectId, depth: usize) -> Result<Arc<IndexedTree>> {
        if depth > 256 {
            return Err(Error::Invalid("snapshot nesting limit reached".into()));
        }
        if let Some(value) = self.cache()?.get(id) {
            return match value {
                Cached::Tree(tree) if depth == 0 || !tree.is_empty() => Ok(tree),
                _ => Err(Error::Invalid(
                    "noncanonical snapshot tree reference".into(),
                )),
            };
        }
        let bytes = self.reader.get(id)?;
        let tree = Arc::new(IndexedTree::decode(bytes)?);
        if depth > 0 && tree.is_empty() {
            return Err(Error::Invalid(
                "noncanonical empty snapshot directory".into(),
            ));
        }
        let weight = tree.accounted_bytes();
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
        let content = self.load_blob(id)?;
        let mut cache = self.cache()?;
        cache.stats.blob_reads += 1;
        cache.insert(id, Cached::Blob(content.clone()), content.len());
        Ok(content)
    }
    fn load_blob(&self, id: ObjectId) -> Result<Arc<[u8]>> {
        let bytes = self.reader.get(id)?;
        let (kind, payload) = decode_object(&bytes)?;
        if kind != BLOB {
            return Err(Error::Invalid("snapshot file references a non-blob".into()));
        }
        Ok(Arc::from(payload))
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
            let Some(entry) = tree.lookup(part) else {
                return Ok(None);
            };
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
        if path.len() > MAX_PAGE_BYTES {
            return Err(Error::Invalid(
                "snapshot path exceeds query size limit".into(),
            ));
        }
        // Only fully validated paths are ever inserted. An exact key hit in
        // this immutable-root view cannot acquire new path semantics.
        if let Some(value) = self.cache()?.get_path(path) {
            return Ok(value);
        }
        let value = match self.resolve(path)? {
            Some((id, TreeEntryKind::File, _)) => self.blob(id).map(Some),
            _ => Ok(None),
        }?;
        self.cache()?.insert_path(path, value.clone());
        Ok(value)
    }

    /// Read a bounded group in input order, including missing paths. Equal
    /// object IDs are fetched once. Independent object loads may run on at
    /// most eight scoped workers when the read provider opts into parallelism.
    /// max_bytes counts returned bytes including repeated paths, not RSS:
    /// per-object read buffers and the separately bounded cache are additional.
    /// Errors fail the whole group before a CLI response is emitted.
    pub fn read_files<P: AsRef<str>>(
        &self,
        paths: &[P],
        max_bytes: usize,
    ) -> Result<Vec<Option<Arc<[u8]>>>> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        if paths.len() > MAX_BULK_PATHS || max_bytes > MAX_BULK_BYTES {
            return Err(Error::Invalid(
                "bulk read exceeds 4096 paths or 64 MiB byte limit".into(),
            ));
        }
        // AsRef implementations may choose a different view on each call.
        // Capture once so validation, lookup, I/O and insertion use one key.
        let paths: Vec<&str> = paths.iter().map(|path| path.as_ref()).collect();
        for &path in &paths {
            if path.len() > MAX_PAGE_BYTES {
                return Err(Error::Invalid(
                    "bulk path size or nesting limit reached".into(),
                ));
            }
        }
        let mut left = max_bytes;
        let cached: Vec<_> = {
            let mut cache = self.cache()?;
            paths
                .iter()
                .map(|&path| {
                    let value = cache.get_path(path);
                    if let Some(Some(bytes)) = &value {
                        left = left.checked_sub(bytes.len()).ok_or_else(|| {
                            Error::Invalid("bulk file content exceeds byte budget".into())
                        })?;
                    }
                    Ok(value)
                })
                .collect::<Result<_>>()?
        };
        // Hits are exact, previously validated keys bound to this root. Check
        // all misses before any tree/object I/O or publishing new path answers.
        for (&path, hit) in paths.iter().zip(cached.iter()) {
            if hit.is_none() {
                validate_path(path)?;
                if path.split('/').count() > 256 {
                    return Err(Error::Invalid(
                        "bulk path size or nesting limit reached".into(),
                    ));
                }
            }
        }
        if cached.iter().all(Option::is_some) {
            return Ok(cached
                .into_iter()
                .map(|value| value.expect("all paths cached"))
                .collect());
        }
        let mut ids = vec![None; paths.len()];
        let mut groups = BTreeMap::<&str, Vec<(usize, &str)>>::new();
        for (index, &path) in paths.iter().enumerate() {
            if cached[index].is_some() {
                continue;
            }
            let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
            groups.entry(parent).or_default().push((index, name));
        }
        // Resolve each requested parent once, keeping its compact directory
        // view for all names in the group, even when the persistent cache is off.
        for (parent, names) in groups {
            let directory = if parent.is_empty() {
                Some((self.root, TreeEntryKind::Directory, 0))
            } else {
                self.resolve(parent)?
            };
            if let Some((id, TreeEntryKind::Directory, depth)) = directory {
                let tree = self.tree(id, depth)?;
                for (index, name) in names {
                    if let Some(entry) = tree.lookup(name) {
                        if entry.kind == TreeEntryKind::File {
                            ids[index] = Some(entry.object);
                        }
                    }
                }
            }
        }
        let mut uses = BTreeMap::<ObjectId, usize>::new();
        for id in ids.iter().flatten() {
            *uses.entry(*id).or_default() += 1;
        }
        let remaining = AtomicUsize::new(left);
        let reserve = |len: usize, count: usize| -> Result<()> {
            let bytes = len
                .checked_mul(count)
                .ok_or_else(|| Error::Invalid("bulk byte count overflow".into()))?;
            let mut left = remaining.load(Ordering::Relaxed);
            loop {
                let next = left.checked_sub(bytes).ok_or_else(|| {
                    Error::Invalid("bulk file content exceeds byte budget".into())
                })?;
                match remaining.compare_exchange_weak(
                    left,
                    next,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return Ok(()),
                    Err(current) => left = current,
                }
            }
        };
        let mut values = HashMap::with_capacity(uses.len());
        let mut work = Vec::with_capacity(uses.len());
        {
            let mut cache = self.cache()?;
            for (id, count) in uses {
                match cache.get(id) {
                    Some(Cached::Blob(content)) => {
                        reserve(content.len(), count)?;
                        values.insert(id, content);
                    }
                    Some(Cached::Tree(_)) => {
                        return Err(Error::Invalid("bulk file references a tree".into()))
                    }
                    None => work.push((id, count)),
                }
            }
        }
        let load = |&(id, count): &(ObjectId, usize)| -> Result<_> {
            let content = self.load_blob(id)?;
            reserve(content.len(), count)?;
            Ok((id, content))
        };
        // CPU/cgroup discovery is unnecessary when all objects are cached or
        // the serial threshold applies. It can itself involve filesystem I/O.
        let workers = if work.len() >= 32 {
            self.reader
                .max_concurrent_reads()
                .clamp(1, 8)
                .min(std::thread::available_parallelism().map_or(1, usize::from))
        } else {
            1
        };
        let loaded: HashMap<_, _> = parallel_map(&work, workers, &load)?.into_iter().collect();
        {
            // Publish verified cache entries together, outside all I/O workers.
            let mut cache = self.cache()?;
            for (id, content) in &loaded {
                cache.stats.blob_reads += 1;
                cache.insert(*id, Cached::Blob(content.clone()), content.len());
            }
        }
        values.extend(loaded);
        let result: Vec<_> = ids
            .into_iter()
            .zip(cached.iter())
            .map(|(id, cached)| {
                if let Some(value) = cached {
                    return Ok(value.clone());
                }
                id.map(|id| {
                    values
                        .get(&id)
                        .cloned()
                        .ok_or_else(|| Error::Invalid("bulk reader omitted an object".into()))
                })
                .transpose()
            })
            .collect::<Result<_>>()?;
        let mut cache = self.cache()?;
        for ((&path, value), cached) in paths.iter().zip(result.iter()).zip(cached.iter()) {
            if cached.is_none() {
                cache.insert_path(path, value.clone());
            }
        }
        Ok(result)
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
                for entry in self.tree(id, depth)?.entries() {
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
    use crate::NativeTree;

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
