//! Revision-pinned queries with a bounded cache of verified immutable content.
//! Cache entries are never consulted for publication, recovery or fsck.
use crate::format::{decode_object, validate_path, BLOB};
use crate::store::ObjectStore;
use crate::tree_index::IndexedTree;
use crate::workers::parallel_map;
use crate::{Error, ObjectId, Result, TreeEntryKind};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

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
    /// Entries not retained because the shared store-wide cache budget was
    /// occupied by other live snapshots.
    pub cache_admission_rejections: u64,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SparseExportStats {
    pub files: usize,
    pub bytes: u64,
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
    shared_memory: Arc<crate::store::SnapshotCacheMemory>,
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
    fn remove_oldest(&mut self) -> bool {
        let Some(oldest) = self.order.pop_front() else {
            return false;
        };
        let removed = match oldest {
            CacheKey::Object(id) => self.entries.remove(&id).map(|(_, size)| size),
            CacheKey::Path(path) => self.paths.remove(&path).map(|entry| entry.weight),
        };
        if let Some(size) = removed {
            self.stats.cache_bytes -= size;
            self.shared_memory.release(size);
        }
        self.update_counts();
        true
    }
    fn reserve(&mut self, weight: usize) -> bool {
        if weight > self.budget {
            return false;
        }
        while self.stats.cache_bytes.saturating_add(weight) > self.budget {
            if !self.remove_oldest() {
                return false;
            }
        }
        while !self.shared_memory.try_acquire(weight) {
            if !self.remove_oldest() {
                self.stats.cache_admission_rejections += 1;
                return false;
            }
        }
        true
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
        if self.paths.contains_key(path) || !self.reserve(weight) {
            return;
        }
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
        if self.entries.contains_key(&id) || !self.reserve(weight) {
            return;
        }
        self.stats.cache_bytes += weight;
        self.entries.insert(id, (value, weight));
        self.order.push_back(CacheKey::Object(id));
        self.update_counts();
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        self.shared_memory.release(self.stats.cache_bytes);
        self.stats.cache_bytes = 0;
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
        let shared_memory = store.snapshot_cache_memory();
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
                shared_memory,
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

    /// Materialize selected immutable snapshot paths into one newly-created
    /// directory without consulting or changing a branch, stage or worktree.
    /// A completion marker is written only after every file is synced. If an
    /// ordinary error occurs the new directory remains explicitly incomplete,
    /// so callers never confuse a partial export with a usable workspace.
    pub fn export_sparse(
        &self,
        destination: impl AsRef<Path>,
        prefixes: &[String],
    ) -> Result<SparseExportStats> {
        const MARKER: &str = ".loom-sparse-complete";
        let destination = destination.as_ref();
        for prefix in prefixes {
            validate_path(prefix)?;
        }
        std::fs::create_dir(destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Error::Invalid("sparse export destination already exists".into())
            } else {
                Error::Io(error)
            }
        })?;

        let requested: Vec<Option<&str>> = if prefixes.is_empty() {
            vec![None]
        } else {
            prefixes
                .iter()
                .map(|prefix| Some(prefix.as_str()))
                .collect()
        };
        let mut paths = std::collections::BTreeSet::new();
        for prefix in requested {
            let mut cursor = None;
            let mut selected = false;
            loop {
                let page = self.files(prefix, cursor.as_deref(), 4096)?;
                selected |= !page.paths.is_empty();
                paths.extend(page.paths);
                let Some(next) = page.next_cursor else {
                    break;
                };
                cursor = Some(next);
            }
            if !selected {
                if let Some(prefix) = prefix {
                    return Err(Error::Invalid(format!(
                        "sparse export prefix selects no files: {prefix}"
                    )));
                }
            }
        }
        if paths.contains(MARKER)
            || paths
                .range(format!("{MARKER}/")..)
                .next()
                .is_some_and(|path| path.starts_with(&format!("{MARKER}/")))
        {
            return Err(Error::Invalid(
                "snapshot collides with sparse export completion marker".into(),
            ));
        }

        let mut bytes = 0u64;
        for path in &paths {
            let content = self.read_file(path)?.ok_or_else(|| {
                Error::Invalid(format!("listed sparse export file disappeared: {path}"))
            })?;
            bytes = bytes
                .checked_add(content.len() as u64)
                .ok_or_else(|| Error::Invalid("sparse export byte count overflow".into()))?;
            let target = destination.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?;
            output.write_all(&content)?;
            output.sync_all()?;
        }
        let mut marker = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination.join(MARKER))?;
        writeln!(marker, "LOOM-SPARSE 1")?;
        writeln!(marker, "root {}", self.root)?;
        writeln!(marker, "files {}", paths.len())?;
        writeln!(marker, "bytes {bytes}")?;
        marker.sync_all()?;
        Ok(SparseExportStats {
            files: paths.len(),
            bytes,
        })
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

    /// Paths have already been validated and captured once by read_files.
    /// This traversal shares storage reads when no objects can be retained.
    fn resolve_uncached_files(&self, paths: &[&str]) -> Result<Vec<Option<ObjectId>>> {
        let mut ids = vec![None; paths.len()];
        let unresolved: Vec<_> = paths
            .iter()
            .enumerate()
            .map(|(index, _)| (index, 0usize))
            .collect();
        let mut frontier = BTreeMap::from([(self.root, unresolved)]);
        let mut depth = 0;
        while !frontier.is_empty() {
            // Share ancestors and directory DAGs at each depth, including
            // zero-cache reads. Keep only slot/offset records between levels,
            // dropping each tree before loading the next: no retained stack
            // or whole layer of potentially large tree buffers.
            let mut children = BTreeMap::<ObjectId, Vec<(usize, usize)>>::new();
            for (id, queries) in frontier {
                let tree = self.tree(id, depth)?;
                for (index, offset) in queries {
                    let suffix = &paths[index][offset..];
                    let (name, rest) = match suffix.split_once('/') {
                        Some((name, rest)) => (name, Some(rest)),
                        None => (suffix, None),
                    };
                    if let Some(entry) = tree.lookup(name) {
                        match (rest, entry.kind) {
                            (None, TreeEntryKind::File) => ids[index] = Some(entry.object),
                            (Some(_), TreeEntryKind::Directory) => {
                                children
                                    .entry(entry.object)
                                    .or_default()
                                    .push((index, offset + name.len() + 1));
                            }
                            _ => {}
                        }
                    }
                }
            }
            frontier = children;
            depth += 1;
        }
        Ok(ids)
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
        let without_cache;
        let cached: Vec<_> = {
            let mut cache = self.cache()?;
            without_cache = cache.budget == 0;
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
        let ids = if without_cache {
            self.resolve_uncached_files(&paths)?
        } else {
            let mut ids = vec![None; paths.len()];
            let mut groups = BTreeMap::<&str, Vec<(usize, &str)>>::new();
            for (index, &path) in paths.iter().enumerate() {
                if cached[index].is_some() {
                    continue;
                }
                let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
                groups.entry(parent).or_default().push((index, name));
            }
            // Retain each parent view for every name in its group.
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
            ids
        };
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
        // CPU/cgroup discovery is unnecessary when all objects are cached,
        // the serial threshold applies, or the provider requests serial I/O.
        let workers = if work.len() >= 32 {
            let hint = self.reader.max_concurrent_reads().clamp(1, 8);
            if hint > 1 {
                hint.min(std::thread::available_parallelism().map_or(1, usize::from))
            } else {
                1
            }
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
    fn snapshot_caches_share_store_memory_and_recover_after_drop() {
        let dir = tempfile::tempdir().unwrap();
        crate::Repository::init(dir.path()).unwrap();
        let store = ObjectStore::new(&dir.path().join(".loom"));
        let blob = store.put(BLOB, b"shared cache budget").unwrap();
        let state = BTreeMap::from([("file".to_owned(), blob)]);
        let root = crate::tree::write_snapshot(&store, &state).unwrap();
        let root_weight = IndexedTree::decode(store.get(root).unwrap())
            .unwrap()
            .accounted_bytes()
            + 128;
        let store = store.with_snapshot_cache_budget(root_weight);

        let first = Snapshot::new(root, store.clone(), root_weight).unwrap();
        assert_eq!(first.stats().unwrap().cache_bytes, root_weight);

        // The second snapshot remains fully usable but cannot multiply the
        // retained root allocation while the first owns the shared budget.
        let second = Snapshot::new(root, store.clone(), root_weight).unwrap();
        let second_stats = second.stats().unwrap();
        assert_eq!(second_stats.cache_bytes, 0);
        assert_eq!(second_stats.cache_admission_rejections, 1);
        assert_eq!(
            second.read_file("file").unwrap().as_deref(),
            Some(&b"shared cache budget"[..])
        );
        drop(first);

        let third = Snapshot::new(root, store, root_weight).unwrap();
        assert_eq!(third.stats().unwrap().cache_bytes, root_weight);
    }

    #[test]
    fn bulk_traversal_shares_ancestors_and_directory_dags_without_a_cache() {
        let dir = tempfile::tempdir().unwrap();
        crate::Repository::init(dir.path()).unwrap();
        let store = ObjectStore::new(&dir.path().join(".loom"));
        let mut state = BTreeMap::new();
        let mut paths = Vec::new();
        for alias in ["left", "right"] {
            for n in 0..96 {
                let blob = store.put(BLOB, format!("value {n}").as_bytes()).unwrap();
                let path = format!("{alias}/上/é/dir{n:03}/file");
                state.insert(path.clone(), blob);
                paths.push(path);
            }
        }
        let root = crate::tree::write_snapshot(&store, &state).unwrap();
        paths.extend([
            "left/上/é/dir000".into(),
            "left/上/é/dir000/file/child".into(),
            "left/上/é/dir000/missing".into(),
            "absent/上/é/dir000/file".into(),
        ]);
        for budget in [0, 512, DEFAULT_SNAPSHOT_CACHE_BYTES] {
            let snapshot = Snapshot::new(root, store.clone(), budget).unwrap();
            let before = snapshot.stats().unwrap();
            let values = snapshot.read_files(&paths, 4096).unwrap();
            for n in 0..96 {
                assert_eq!(values[n].as_deref(), Some(format!("value {n}").as_bytes()));
                assert!(Arc::ptr_eq(
                    values[n].as_ref().unwrap(),
                    values[n + 96].as_ref().unwrap()
                ));
            }
            assert!(values[192..].iter().all(Option::is_none));
            let after = snapshot.stats().unwrap();
            // Root + three shared ancestors + 96 distinct leaves. Both aliases
            // share the same DAG, even when none of it fits the cache.
            if budget == 0 {
                assert_eq!(after.tree_reads - before.tree_reads, 100);
            }
            assert_eq!(after.blob_reads, 96);
            assert!(after.cache_bytes <= budget);
            assert!(snapshot.read_files(&paths, 1).is_err());
        }
        // Corrupt one leaf of the shared directory layer. Successful siblings
        // may populate the object cache, but no partial path answers escape.
        let mut id = root;
        for component in ["left", "上", "é", "dir000"] {
            id = NativeTree::decode(&store.get(id).unwrap())
                .unwrap()
                .entries
                .into_iter()
                .find(|entry| entry.name == component)
                .unwrap()
                .object;
        }
        let hex = id.to_string();
        let path = dir
            .path()
            .join(".loom/objects")
            .join(&hex[..2])
            .join(&hex[2..]);
        let original = std::fs::read(&path).unwrap();
        let snapshot = Snapshot::new(root, store, DEFAULT_SNAPSHOT_CACHE_BYTES).unwrap();
        std::fs::write(&path, b"corrupt tree").unwrap();
        assert!(snapshot.read_files(&paths, 4096).is_err());
        assert_eq!(snapshot.stats().unwrap().path_cache_entries, 0);
        std::fs::write(&path, original).unwrap();
        assert_eq!(
            snapshot.read_files(&paths, 4096).unwrap().len(),
            paths.len()
        );
    }

    #[test]
    fn bulk_traversal_handles_one_tree_at_different_depths_and_invalid_directories() {
        let dir = tempfile::tempdir().unwrap();
        crate::Repository::init(dir.path()).unwrap();
        let store = ObjectStore::new(&dir.path().join(".loom"));
        let blob = store.put(BLOB, b"shared bytes").unwrap();
        let write_tree = |entries: &[(&str, ObjectId, TreeEntryKind)]| {
            let encoded = NativeTree {
                entries: entries
                    .iter()
                    .map(|&(name, object, kind)| crate::TreeEntry {
                        name: name.into(),
                        object,
                        kind,
                        executable: false,
                    })
                    .collect(),
            }
            .encode()
            .unwrap();
            store
                .put(crate::format::TREE, decode_object(&encoded).unwrap().1)
                .unwrap()
        };
        let leaf = write_tree(&[("file", blob, TreeEntryKind::File)]);
        let branch = write_tree(&[("alias", leaf, TreeEntryKind::Directory)]);
        let empty = write_tree(&[]);
        let root = write_tree(&[
            ("deep", branch, TreeEntryKind::Directory),
            ("empty", empty, TreeEntryKind::Directory),
            ("shallow", leaf, TreeEntryKind::Directory),
            ("wrong", blob, TreeEntryKind::Directory),
        ]);
        let snapshot = Snapshot::new(root, store, 0).unwrap();
        let values = snapshot
            .read_files(
                &[
                    "shallow/file",
                    "deep/alias/file",
                    "shallow",
                    "shallow/file/child",
                    "deep/missing/file",
                ],
                24,
            )
            .unwrap();
        assert_eq!(values[0].as_deref(), Some(b"shared bytes".as_slice()));
        assert!(Arc::ptr_eq(
            values[0].as_ref().unwrap(),
            values[1].as_ref().unwrap()
        ));
        assert!(values[2..].iter().all(Option::is_none));
        assert!(snapshot.read_files(&["empty/file"], 100).is_err());
        assert!(snapshot.read_files(&["wrong/file"], 100).is_err());
        assert_eq!(snapshot.stats().unwrap().cache_bytes, 0);
    }

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
                let uncached = Snapshot::new(root, store.clone(), 0).unwrap();
                assert_eq!(
                    uncached.read_files(&[path.as_str()], 4).unwrap()[0].as_deref(),
                    Some(b"deep".as_slice())
                );
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
        assert!(snapshot
            .read_files(&[format!("{}z", "a/".repeat(256))], 4)
            .is_err());
    }

    #[test]
    fn sparse_export_is_revision_pinned_deduplicated_and_complete() {
        let dir = tempfile::tempdir().unwrap();
        crate::Repository::init(dir.path()).unwrap();
        let store = ObjectStore::new(&dir.path().join(".loom"));
        let mut state = BTreeMap::new();
        for (path, content) in [
            ("README.md", b"read me".as_slice()),
            ("src/a.rs", b"fn a() {}".as_slice()),
            ("src/deep/b.rs", b"fn b() {}".as_slice()),
            ("tests/t.rs", b"test".as_slice()),
        ] {
            state.insert(path.to_owned(), store.put(BLOB, content).unwrap());
        }
        let root = crate::tree::write_snapshot(&store, &state).unwrap();
        let snapshot = Snapshot::new(root, store, 0).unwrap();
        let destination = dir.path().join("agent-workspace");
        let stats = snapshot
            .export_sparse(
                &destination,
                &["src".into(), "src/deep".into(), "README.md".into()],
            )
            .unwrap();

        assert_eq!(stats.files, 3);
        assert_eq!(stats.bytes, 7 + 9 + 9);
        assert_eq!(
            std::fs::read(destination.join("README.md")).unwrap(),
            b"read me"
        );
        assert_eq!(
            std::fs::read(destination.join("src/a.rs")).unwrap(),
            b"fn a() {}"
        );
        assert_eq!(
            std::fs::read(destination.join("src/deep/b.rs")).unwrap(),
            b"fn b() {}"
        );
        assert!(!destination.join("tests").exists());
        let marker = std::fs::read_to_string(destination.join(".loom-sparse-complete")).unwrap();
        assert!(marker.contains(&format!("root {root}")));
        assert!(marker.contains("files 3"));
    }

    #[test]
    fn sparse_export_never_reuses_a_destination_or_completes_a_missing_selection() {
        let dir = tempfile::tempdir().unwrap();
        crate::Repository::init(dir.path()).unwrap();
        let store = ObjectStore::new(&dir.path().join(".loom"));
        let blob = store.put(BLOB, b"value").unwrap();
        let root =
            crate::tree::write_snapshot(&store, &BTreeMap::from([("file".into(), blob)])).unwrap();
        let snapshot = Snapshot::new(root, store, 0).unwrap();

        let existing = dir.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        std::fs::write(existing.join("owned"), b"keep").unwrap();
        assert!(snapshot.export_sparse(&existing, &[]).is_err());
        assert_eq!(std::fs::read(existing.join("owned")).unwrap(), b"keep");

        let missing = dir.path().join("missing-selection");
        assert!(snapshot
            .export_sparse(&missing, &["absent".into()])
            .is_err());
        assert!(missing.is_dir());
        assert!(!missing.join(".loom-sparse-complete").exists());
    }
}
