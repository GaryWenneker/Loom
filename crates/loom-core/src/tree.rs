//! Canonical, immutable, hierarchical directory nodes.
//!
//! This is a version-2 *object type*, not a migration of the v1 changeset
//! history or a claim that v2 worktree metadata is already preserved.
use crate::format::{decode_object, encode_object, validate_path, ObjectId, TREE};
use crate::{Error, Result};

pub(crate) const MAX_ENTRIES: usize = 1_000_000;
pub(crate) const MAX_NAME: usize = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeEntryKind {
    File = 1,
    Directory = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// One path component, never a path with separators.
    pub name: String,
    pub kind: TreeEntryKind,
    /// Reserved for preserving the executable bit; v1 migrations use false.
    pub executable: bool,
    pub object: ObjectId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTree {
    /// Lexicographically sorted by UTF-8 bytes; no duplicate names.
    pub entries: Vec<TreeEntry>,
}

pub(crate) fn validate_entry_parts(
    name: &str,
    kind: TreeEntryKind,
    executable: bool,
) -> Result<()> {
    validate_path(name)?;
    if name.contains('/') || name.len() > MAX_NAME {
        return Err(Error::Invalid(
            "tree name must be one bounded path component".into(),
        ));
    }
    if kind == TreeEntryKind::Directory && executable {
        return Err(Error::Invalid(
            "directory cannot have executable-file flag".into(),
        ));
    }
    Ok(())
}

impl NativeTree {
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.entries.len() > MAX_ENTRIES {
            return Err(Error::Invalid("too many tree entries".into()));
        }
        let mut data = Vec::new();
        data.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        let mut previous: Option<&str> = None;
        for entry in &self.entries {
            validate_entry_parts(&entry.name, entry.kind, entry.executable)?;
            if previous.is_some_and(|name| name >= entry.name.as_str()) {
                return Err(Error::Invalid("unsorted or repeated tree entry".into()));
            }
            previous = Some(&entry.name);
            data.push(entry.kind as u8);
            data.push(u8::from(entry.executable));
            data.extend_from_slice(&(entry.name.len() as u32).to_le_bytes());
            data.extend_from_slice(entry.name.as_bytes());
            data.extend_from_slice(&entry.object.0);
        }
        encode_object(TREE, &data)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let parsed = crate::tree_index::TreeData::decode(bytes)?;
        Ok(Self {
            entries: parsed
                .entries()
                .map(|entry| TreeEntry {
                    name: entry.name.to_owned(),
                    kind: entry.kind,
                    executable: entry.executable,
                    object: entry.object,
                })
                .collect(),
        })
    }
}

/// Assemble one canonical directory DAG from the immutable file state.
/// The conversion is optional and has no effect on existing v1 commits.
pub(crate) fn write_snapshot(
    store: &crate::store::ObjectStore,
    state: &std::collections::BTreeMap<String, ObjectId>,
) -> Result<ObjectId> {
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Folder {
        files: BTreeMap<String, ObjectId>,
        dirs: BTreeMap<String, Folder>,
    }
    fn save(store: &crate::store::ObjectStore, folder: &Folder, depth: usize) -> Result<ObjectId> {
        if depth > 256 {
            return Err(Error::Invalid("native tree nesting limit reached".into()));
        }
        let mut entries = Vec::new();
        for (name, blob) in &folder.files {
            entries.push(TreeEntry {
                name: name.clone(),
                kind: TreeEntryKind::File,
                executable: false,
                object: *blob,
            });
        }
        for (name, subdir) in &folder.dirs {
            let child = save(store, subdir, depth + 1)?;
            entries.push(TreeEntry {
                name: name.clone(),
                kind: TreeEntryKind::Directory,
                executable: false,
                object: child,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let envelope = NativeTree { entries }.encode()?;
        let (_, payload) = decode_object(&envelope)?;
        store.put(TREE, payload)
    }

    // Validate the complete blob closure before publishing any tree node.
    // A missing object or a tree masquerading as a file must fail closed.
    for (path, blob) in state {
        validate_path(path)?;
        let object = store.get(*blob)?;
        if decode_object(&object)?.0 != crate::format::BLOB {
            return Err(Error::Invalid(format!(
                "native tree file does not reference a blob: {path}"
            )));
        }
    }

    let mut root = Folder::default();
    for (path, blob) in state {
        validate_path(path)?;
        let components: Vec<&str> = path.split('/').collect();
        if components.len() > 256 {
            return Err(Error::Invalid(
                "native tree path nesting limit reached".into(),
            ));
        }
        let mut folder = &mut root;
        for name in &components[..components.len() - 1] {
            if folder.files.contains_key(*name) {
                return Err(Error::Invalid(format!("file/directory collision: {path}")));
            }
            folder = folder.dirs.entry((*name).to_owned()).or_default();
        }
        let name = components.last().expect("validated nonempty path");
        if folder.dirs.contains_key(*name)
            || folder.files.insert((*name).to_owned(), *blob).is_some()
        {
            return Err(Error::Invalid(format!("file/directory collision: {path}")));
        }
    }
    save(store, &root, 0)
}

/// Resolve a tracked file directly from a native tree root without flattening
/// the entire repository. Only directory objects on the requested path are
/// loaded; unrelated subtrees and blobs remain untouched.
///
/// Returns None for a missing file, including when a path component names a
/// regular file rather than a directory.
pub(crate) fn lookup_file(
    store: &crate::store::ObjectStore,
    root: ObjectId,
    path: &str,
) -> Result<Option<ObjectId>> {
    validate_path(path)?;
    let components: Vec<&str> = path.split('/').collect();
    if components.len() > 256 {
        return Err(Error::Invalid(
            "native tree path nesting limit reached".into(),
        ));
    }
    let mut current = root;
    for (index, name) in components.iter().enumerate() {
        let node = NativeTree::decode(&store.get(current)?)?;
        if index > 0 && node.entries.is_empty() {
            return Err(Error::Invalid(
                "noncanonical empty native tree directory".into(),
            ));
        }
        let Ok(position) = node
            .entries
            .binary_search_by(|entry| entry.name.as_str().cmp(name))
        else {
            return Ok(None);
        };
        let entry = &node.entries[position];
        let last = index + 1 == components.len();
        match (last, entry.kind) {
            (true, TreeEntryKind::File) => return Ok(Some(entry.object)),
            (true, TreeEntryKind::Directory) => return Ok(None),
            (false, TreeEntryKind::File) => return Ok(None),
            (false, TreeEntryKind::Directory) => current = entry.object,
        }
    }
    Ok(None)
}

/// List tracked files beneath a directory prefix without opening sibling
/// subtrees outside the prefix. The result is ordered by canonical path.
pub(crate) fn list_prefix(
    store: &crate::store::ObjectStore,
    root: ObjectId,
    prefix: &str,
) -> Result<std::collections::BTreeMap<String, ObjectId>> {
    use std::collections::BTreeMap;

    fn visit(
        store: &crate::store::ObjectStore,
        id: ObjectId,
        base: &str,
        depth: usize,
        result: &mut BTreeMap<String, ObjectId>,
    ) -> Result<()> {
        if depth > 256 {
            return Err(Error::Invalid("native tree nesting limit reached".into()));
        }
        let tree = NativeTree::decode(&store.get(id)?)?;
        if depth > 0 && tree.entries.is_empty() {
            return Err(Error::Invalid(
                "noncanonical empty native tree directory".into(),
            ));
        }
        for entry in tree.entries {
            let path = if base.is_empty() {
                entry.name
            } else {
                format!("{base}/{}", entry.name)
            };
            match entry.kind {
                TreeEntryKind::File => {
                    result.insert(path, entry.object);
                }
                TreeEntryKind::Directory => {
                    visit(store, entry.object, &path, depth + 1, result)?;
                }
            }
        }
        Ok(())
    }

    let mut current = root;
    let mut base = String::new();
    if !prefix.is_empty() {
        validate_path(prefix)?;
        let parts: Vec<&str> = prefix.split('/').collect();
        if parts.len() > 256 {
            return Err(Error::Invalid(
                "native tree path nesting limit reached".into(),
            ));
        }
        for (index, part) in parts.iter().enumerate() {
            let tree = NativeTree::decode(&store.get(current)?)?;
            if index > 0 && tree.entries.is_empty() {
                return Err(Error::Invalid(
                    "noncanonical empty native tree directory".into(),
                ));
            }
            let Ok(position) = tree.entries.binary_search_by(|e| e.name.as_str().cmp(part)) else {
                return Ok(BTreeMap::new());
            };
            let entry = &tree.entries[position];
            base = if base.is_empty() {
                (*part).to_owned()
            } else {
                format!("{base}/{part}")
            };
            if entry.kind == TreeEntryKind::File {
                let mut result = BTreeMap::new();
                if index + 1 == parts.len() {
                    result.insert(base, entry.object);
                }
                return Ok(result);
            }
            current = entry.object;
        }
    }
    let mut result = BTreeMap::new();
    visit(
        store,
        current,
        &base,
        prefix.split('/').filter(|p| !p.is_empty()).count(),
        &mut result,
    )?;
    Ok(result)
}

/// Apply a changeset to a canonical immutable directory DAG. Only
/// directories on paths touched by the changeset are opened and re-encoded.
/// Unchanged sibling objects retain their IDs, including across revisions.
///
/// Changes are batched into one edit trie, so several edits in the same
/// directory produce one new directory object, not one per changed file.
pub(crate) fn apply_changes(
    store: &crate::store::ObjectStore,
    root: ObjectId,
    changes: &[crate::format::Change],
) -> Result<ObjectId> {
    apply_changes_with_workers(store, root, changes, 0)
}

/// Apply one edit trie while independently building changed subtrees.
///
/// The worker request uses the same zero/explicit bounds as worktree scans.
/// Follow shared prefixes to independent directory edits. Descendants of a
/// fork remain serial, preventing nested thread multiplication. Provider write
/// limits are respected; parents are persisted in canonical name order.
pub(crate) fn apply_changes_with_workers(
    store: &crate::store::ObjectStore,
    root: ObjectId,
    changes: &[crate::format::Change],
    requested_workers: usize,
) -> Result<ObjectId> {
    use crate::format::{Change, ChangeKind};
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Edits {
        value: Option<Option<ObjectId>>,
        children: BTreeMap<String, Edits>,
    }

    impl Edits {
        fn has_additions(&self) -> bool {
            self.value.is_some_and(|v| v.is_some())
                || self.children.values().any(Edits::has_additions)
        }
    }

    fn persist(store: &crate::store::ObjectStore, entries: Vec<TreeEntry>) -> Result<ObjectId> {
        let envelope = NativeTree { entries }.encode()?;
        let (_, payload) = decode_object(&envelope)?;
        store.put(TREE, payload)
    }

    fn apply_child(
        store: &crate::store::ObjectStore,
        name: &str,
        existing: Option<TreeEntry>,
        child: &Edits,
        depth: usize,
        workers: usize,
    ) -> Result<Option<TreeEntry>> {
        let replacement = match child.value {
            Some(Some(blob)) => {
                // A file cannot contain another added file. When replacing an
                // existing directory with a file, every descendant must be
                // explicitly deleted.
                if child.children.values().any(Edits::has_additions) {
                    return Err(Error::Invalid(format!(
                        "file/directory collision at native tree entry: {name}"
                    )));
                }
                if let Some(previous) = &existing {
                    if previous.kind == TreeEntryKind::File && !child.children.is_empty() {
                        return Err(Error::Invalid(format!(
                            "native tree deletion beneath a file: {name}"
                        )));
                    }
                    if previous.kind == TreeEntryKind::Directory
                        && apply_dir(store, Some(previous.object), child, depth + 1, workers)?
                            .is_some()
                    {
                        return Err(Error::Invalid(format!(
                            "native tree directory has undeleted descendants: {name}"
                        )));
                    }
                }
                Some(TreeEntry {
                    name: name.to_owned(),
                    kind: TreeEntryKind::File,
                    // Modifying file contents must not silently discard the
                    // executable metadata stored in the native tree.
                    executable: existing
                        .as_ref()
                        .is_some_and(|entry| entry.kind == TreeEntryKind::File && entry.executable),
                    object: blob,
                })
            }
            Some(None) => {
                // A directory cannot be deleted as if it were a file in the
                // v1 changeset model. A removed file can instead be replaced
                // with a directory in the same batch.
                if existing
                    .as_ref()
                    .is_some_and(|entry| entry.kind == TreeEntryKind::Directory)
                {
                    return Err(Error::Invalid(format!(
                        "native tree deletion targets a directory: {name}"
                    )));
                }
                apply_dir(store, None, child, depth + 1, workers)?.map(|id| TreeEntry {
                    name: name.to_owned(),
                    kind: TreeEntryKind::Directory,
                    executable: false,
                    object: id,
                })
            }
            None => {
                let prior = match existing {
                    Some(entry) if entry.kind == TreeEntryKind::Directory => Some(entry.object),
                    Some(_) => {
                        return Err(Error::Invalid(format!(
                            "cannot add path beneath a native tree file: {name}"
                        )))
                    }
                    None => None,
                };
                apply_dir(store, prior, child, depth + 1, workers)?.map(|id| TreeEntry {
                    name: name.to_owned(),
                    kind: TreeEntryKind::Directory,
                    executable: false,
                    object: id,
                })
            }
        };
        Ok(replacement)
    }

    fn apply_dir(
        store: &crate::store::ObjectStore,
        previous: Option<ObjectId>,
        edits: &Edits,
        depth: usize,
        workers: usize,
    ) -> Result<Option<ObjectId>> {
        if depth > 256 {
            return Err(Error::Invalid("native tree nesting limit reached".into()));
        }
        let mut entries: BTreeMap<String, TreeEntry> = if let Some(id) = previous {
            let tree = NativeTree::decode(&store.get(id)?)?;
            if depth > 0 && tree.entries.is_empty() {
                return Err(Error::Invalid(
                    "noncanonical empty native tree directory".into(),
                ));
            }
            tree.entries
                .into_iter()
                .map(|entry| (entry.name.clone(), entry))
                .collect()
        } else {
            BTreeMap::new()
        };

        // File-entry edits stay serial. Group directory rebuilds separately
        // so a few expensive tasks cannot hide in one chunk of cheap work.
        let (subtrees, files): (Vec<_>, Vec<_>) = edits
            .children
            .iter()
            .map(|(name, child)| (name.as_str(), entries.remove(name), child))
            .partition(|(_, _, child)| !child.children.is_empty());
        for (name, existing, child) in files {
            if let Some(entry) = apply_child(store, name, existing, child, depth, 1)? {
                entries.insert(name.to_owned(), entry);
            }
        }
        let fork = workers > 1 && subtrees.len() >= 2;
        let descendants = if fork { 1 } else { workers };
        let replacements = crate::workers::parallel_map_min(
            &subtrees,
            if fork { workers } else { 1 },
            2,
            &|(name, existing, child)| {
                apply_child(store, name, existing.clone(), child, depth, descendants)
            },
        )?;
        // Every worker is joined before canonical parent publication. Failed
        // builds may leave only unreachable immutable objects.
        for ((name, _, _), replacement) in subtrees.into_iter().zip(replacements) {
            if let Some(entry) = replacement {
                entries.insert(name.to_owned(), entry);
            }
        }
        if entries.is_empty() {
            return Ok(None);
        }
        Ok(Some(persist(store, entries.into_values().collect())?))
    }

    let mut edits = Edits::default();
    for Change { path, kind, blob } in changes {
        validate_path(path)?;
        let components: Vec<&str> = path.split('/').collect();
        if components.len() > 256 {
            return Err(Error::Invalid(
                "native tree path nesting limit reached".into(),
            ));
        }
        let value = match kind {
            ChangeKind::Delete if blob.is_none() => None,
            ChangeKind::Add | ChangeKind::Modify if blob.is_some() => {
                let oid = blob.expect("checked presence");
                let bytes = store.get(oid)?;
                if decode_object(&bytes)?.0 != crate::format::BLOB {
                    return Err(Error::Invalid(format!(
                        "native tree update references a non-blob: {path}"
                    )));
                }
                Some(oid)
            }
            _ => {
                return Err(Error::Invalid(format!(
                    "invalid native tree change: {path}"
                )))
            }
        };
        let mut node = &mut edits;
        for component in components {
            node = node.children.entry(component.to_owned()).or_default();
        }
        if node.value.replace(value).is_some() {
            return Err(Error::Invalid(format!(
                "duplicate native tree change path: {path}"
            )));
        }
    }
    if changes.is_empty() {
        NativeTree::decode(&store.get(root)?)?;
        return Ok(root);
    }
    let workers = crate::worktree_scan::worker_limit(
        requested_workers,
        std::thread::available_parallelism().map_or(1, usize::from),
    )
    .min(store.max_concurrent_writes());
    // The empty root has an object ID too. Empty nonroot directories are
    // pruned, ensuring canonical output matches write_snapshot().
    let result = apply_dir(store, Some(root), &edits, 0, workers)?;
    match result {
        Some(id) => Ok(id),
        None => persist(store, Vec::new()),
    }
}

/// Traverse native trees with bounded depth and validate every referenced
/// blob/child tree. This is used by integrity checks and format conformance.
pub(crate) fn flatten_snapshot(
    store: &crate::store::ObjectStore,
    root: ObjectId,
) -> Result<std::collections::BTreeMap<String, ObjectId>> {
    use std::collections::{BTreeMap, HashSet};
    fn visit(
        store: &crate::store::ObjectStore,
        id: ObjectId,
        prefix: &str,
        depth: usize,
        ancestry: &mut HashSet<ObjectId>,
        state: &mut BTreeMap<String, ObjectId>,
    ) -> Result<()> {
        if depth > 256 || !ancestry.insert(id) {
            return Err(Error::Invalid(
                "native tree depth or cycle limit reached".into(),
            ));
        }
        let result = (|| -> Result<()> {
            let node = NativeTree::decode(&store.get(id)?)?;
            if depth > 0 && node.entries.is_empty() {
                return Err(Error::Invalid(
                    "noncanonical empty native tree directory".into(),
                ));
            }
            for entry in node.entries {
                let path = if prefix.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{prefix}/{}", entry.name)
                };
                validate_path(&path)?;
                match entry.kind {
                    TreeEntryKind::File => {
                        let data = store.get(entry.object)?;
                        if decode_object(&data)?.0 != crate::format::BLOB {
                            return Err(Error::Invalid(format!(
                                "native tree file does not reference a blob: {path}"
                            )));
                        }
                        if state.insert(path, entry.object).is_some() {
                            return Err(Error::Invalid("duplicate native tree path".into()));
                        }
                    }
                    TreeEntryKind::Directory => {
                        visit(store, entry.object, &path, depth + 1, ancestry, state)?;
                    }
                }
            }
            Ok(())
        })();
        ancestry.remove(&id);
        result
    }
    let mut state = BTreeMap::new();
    visit(store, root, "", 0, &mut HashSet::new(), &mut state)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage_backend::{ObjectBackend, StoreCapabilities};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};

    struct PublicationProbe {
        inner: crate::FilesystemObjectBackend,
        limit: usize,
        observe: AtomicBool,
        fail: AtomicBool,
        active: AtomicUsize,
        peak: AtomicUsize,
        overlap: (Mutex<bool>, Condvar),
    }

    impl PublicationProbe {
        fn new(path: &std::path::Path, limit: usize) -> Self {
            Self {
                inner: crate::FilesystemObjectBackend::open(path).unwrap(),
                limit,
                observe: AtomicBool::new(false),
                fail: AtomicBool::new(false),
                active: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                overlap: (Mutex::new(false), Condvar::new()),
            }
        }
    }

    impl ObjectBackend for PublicationProbe {
        fn capabilities(&self) -> StoreCapabilities {
            self.inner.capabilities()
        }
        fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
            self.inner.get(id)
        }
        fn exists(&self, id: ObjectId) -> Result<bool> {
            self.inner.exists(id)
        }
        fn list_ids(&self) -> Result<Vec<ObjectId>> {
            self.inner.list_ids()
        }
        fn max_concurrent_writes(&self) -> usize {
            self.limit
        }
        fn put_if_absent(&self, id: ObjectId, bytes: &[u8]) -> Result<()> {
            if !self.observe.load(Ordering::SeqCst) || decode_object(bytes)?.0 != TREE {
                return self.inner.put_if_absent(id, bytes);
            }
            struct Active<'a>(&'a AtomicUsize);
            impl Drop for Active<'_> {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let count = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            let _active = Active(&self.active);
            self.peak.fetch_max(count, Ordering::SeqCst);
            if count > self.limit {
                return Err(Error::Invalid("provider write limit exceeded".into()));
            }
            if self.fail.load(Ordering::SeqCst) {
                return Err(Error::Invalid(
                    "injected native-tree publication failure".into(),
                ));
            }
            let tree = NativeTree::decode(bytes)?;
            let leaf = tree.entries.len() == 1
                && tree.entries[0].name == "file"
                && tree.entries[0].kind == TreeEntryKind::File;
            if self.limit > 1
                && std::thread::available_parallelism().map_or(1, usize::from) > 1
                && leaf
            {
                let (reached, wake) = &self.overlap;
                let mut reached = reached.lock().unwrap();
                if count >= 2 {
                    *reached = true;
                    wake.notify_all();
                }
                let (reached, _) = wake
                    .wait_timeout_while(reached, std::time::Duration::from_secs(5), |value| !*value)
                    .unwrap();
                if !*reached {
                    return Err(Error::Invalid(
                        "independent nested subtrees did not overlap".into(),
                    ));
                }
            }
            self.inner.put_if_absent(id, bytes)
        }
    }

    fn tree() -> NativeTree {
        NativeTree {
            entries: vec![
                TreeEntry {
                    name: "README.md".into(),
                    kind: TreeEntryKind::File,
                    executable: false,
                    object: ObjectId([1; 32]),
                },
                TreeEntry {
                    name: "src".into(),
                    kind: TreeEntryKind::Directory,
                    executable: false,
                    object: ObjectId([2; 32]),
                },
            ],
        }
    }

    // Ten portable serialization edge cases, kept in one CI-reviewed change.
    #[test]
    fn deleting_descendant_of_existing_file_is_rejected() {
        use crate::format::{Change, ChangeKind, BLOB};
        use std::{collections::BTreeMap, fs};
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        fs::create_dir(&meta).unwrap();
        fs::create_dir(meta.join("objects")).unwrap();
        let store = crate::store::ObjectStore::new(&meta);
        let blob = store.put(BLOB, b"content").unwrap();
        let root = write_snapshot(&store, &BTreeMap::from([("parent".to_owned(), blob)])).unwrap();
        let changes = [Change {
            path: "parent/child".to_owned(),
            kind: ChangeKind::Delete,
            blob: None,
        }];
        assert!(apply_changes(&store, root, &changes).is_err());
        // A replacement at the parent must not shadow a malformed deletion.
        let mut shadowed = changes.to_vec();
        shadowed.push(Change {
            path: "parent".to_owned(),
            kind: ChangeKind::Modify,
            blob: Some(blob),
        });
        assert!(apply_changes(&store, root, &shadowed).is_err());
        // Explicit removal of the parent permits a valid file-to-directory transition.
        let transition = [
            Change {
                path: "parent".to_owned(),
                kind: ChangeKind::Delete,
                blob: None,
            },
            Change {
                path: "parent/child".to_owned(),
                kind: ChangeKind::Add,
                blob: Some(blob),
            },
        ];
        let next = apply_changes(&store, root, &transition).unwrap();
        assert_eq!(
            lookup_file(&store, next, "parent/child").unwrap(),
            Some(blob)
        );
        assert_eq!(lookup_file(&store, root, "parent").unwrap(), Some(blob));
    }

    #[test]
    fn conformance_01_empty_root_roundtrip() {
        let node = NativeTree { entries: vec![] };
        assert_eq!(NativeTree::decode(&node.encode().unwrap()).unwrap(), node);
    }

    #[test]
    fn conformance_02_single_file_roundtrip() {
        let node = NativeTree {
            entries: vec![tree().entries[0].clone()],
        };
        assert_eq!(NativeTree::decode(&node.encode().unwrap()).unwrap(), node);
    }

    #[test]
    fn conformance_03_unicode_component_roundtrip() {
        let mut node = tree();
        node.entries[0].name = "café.txt".into();
        assert_eq!(NativeTree::decode(&node.encode().unwrap()).unwrap(), node);
    }

    #[test]
    fn conformance_04_case_distinct_entries() {
        let mut node = tree();
        node.entries[0].name = "Readme".into();
        node.entries[1].name = "readme".into();
        assert_eq!(NativeTree::decode(&node.encode().unwrap()).unwrap(), node);
    }

    #[test]
    fn conformance_05_repeated_name_rejected() {
        let mut node = tree();
        node.entries[1].name = node.entries[0].name.clone();
        assert!(node.encode().is_err());
    }

    #[test]
    fn conformance_06_reverse_order_rejected() {
        let mut node = tree();
        node.entries.reverse();
        assert!(node.encode().is_err());
    }

    #[test]
    fn conformance_07_backslash_component_rejected() {
        let mut node = tree();
        node.entries[0].name = "dir\\file".into();
        assert!(node.encode().is_err());
    }

    #[test]
    fn conformance_08_executable_directory_rejected() {
        let mut node = tree();
        node.entries[1].executable = true;
        assert!(node.encode().is_err());
    }

    #[test]
    fn conformance_09_trailing_payload_rejected() {
        let mut encoded = tree().encode().unwrap();
        encoded.push(0);
        assert!(NativeTree::decode(&encoded).is_err());
    }

    #[test]
    fn conformance_10_truncated_payload_rejected() {
        let encoded = tree().encode().unwrap();
        assert!(NativeTree::decode(&encoded[..encoded.len() - 1]).is_err());
    }

    #[test]
    fn sparse_lookup_resolves_files_without_scanning_siblings() {
        use crate::format::BLOB;
        use std::collections::BTreeMap;
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        fs::create_dir(&meta).unwrap();
        fs::create_dir(meta.join("objects")).unwrap();
        let store = crate::store::ObjectStore::new(&meta);
        let blob = store.put(BLOB, b"target").unwrap();
        let other = store.put(BLOB, b"other").unwrap();
        let root = write_snapshot(
            &store,
            &BTreeMap::from([
                ("src/nested/target.rs".to_string(), blob),
                ("unrelated/other.rs".to_string(), other),
            ]),
        )
        .unwrap();
        assert_eq!(
            lookup_file(&store, root, "src/nested/target.rs").unwrap(),
            Some(blob)
        );
        assert_eq!(
            lookup_file(&store, root, "src/nested/missing.rs").unwrap(),
            None
        );
        assert_eq!(lookup_file(&store, root, "src/nested").unwrap(), None);
        assert_eq!(
            lookup_file(&store, root, "src/nested/target.rs/child").unwrap(),
            None
        );
        assert!(lookup_file(&store, root, "../escape").is_err());
        // A missing unrelated subtree must not break lookup of another path.
        let mut entries = NativeTree::decode(&store.get(root).unwrap())
            .unwrap()
            .entries;
        let sibling = entries.iter_mut().find(|e| e.name == "unrelated").unwrap();
        sibling.object = ObjectId([0xEE; 32]);
        let bytes = NativeTree { entries }.encode().unwrap();
        let (_, payload) = decode_object(&bytes).unwrap();
        let sparse_root = store.put(TREE, payload).unwrap();
        assert_eq!(
            lookup_file(&store, sparse_root, "src/nested/target.rs").unwrap(),
            Some(blob)
        );
    }

    #[test]
    fn prefix_listing_skips_unrelated_corrupt_subtrees() {
        use crate::format::BLOB;
        use std::collections::BTreeMap;
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        fs::create_dir(&meta).unwrap();
        fs::create_dir(meta.join("objects")).unwrap();
        let store = crate::store::ObjectStore::new(&meta);
        let blob = store.put(BLOB, b"source").unwrap();
        let root = write_snapshot(
            &store,
            &BTreeMap::from([
                ("src/a.rs".to_string(), blob),
                ("src/deep/b.rs".to_string(), blob),
                ("vendor/unused.rs".to_string(), blob),
            ]),
        )
        .unwrap();
        let expected = BTreeMap::from([
            ("src/a.rs".to_string(), blob),
            ("src/deep/b.rs".to_string(), blob),
        ]);
        assert_eq!(list_prefix(&store, root, "src").unwrap(), expected);
        assert_eq!(
            list_prefix(&store, root, "src/a.rs").unwrap(),
            BTreeMap::from([("src/a.rs".to_string(), blob)])
        );
        assert!(list_prefix(&store, root, "absent").unwrap().is_empty());
        assert!(list_prefix(&store, root, "../escape").is_err());

        let mut entries = NativeTree::decode(&store.get(root).unwrap())
            .unwrap()
            .entries;
        entries
            .iter_mut()
            .find(|e| e.name == "vendor")
            .unwrap()
            .object = ObjectId([0xFD; 32]);
        let envelope = NativeTree { entries }.encode().unwrap();
        let (_, payload) = decode_object(&envelope).unwrap();
        let sparse_root = store.put(TREE, payload).unwrap();
        assert_eq!(list_prefix(&store, sparse_root, "src").unwrap(), expected);
        assert!(list_prefix(&store, sparse_root, "").is_err());
    }

    #[test]
    fn twenty_native_tree_component_conformance_cases() {
        // Ten portable valid UTF-8 names; each must preserve its exact bytes.
        let valid = [
            "a", "README", "file.txt", "A-b_1", "space name",
            "café", "日本語", "Δοκιμή", "é", "data.json",
        ];
        for (case, name) in valid.iter().enumerate() {
            let node = NativeTree { entries: vec![TreeEntry {
                name: (*name).into(),
                kind: TreeEntryKind::File,
                executable: false,
                object: ObjectId([case as u8; 32]),
            }] };
            let encoded = node.encode().unwrap();
            assert_eq!(NativeTree::decode(&encoded).unwrap(), node, "valid case {case}");
        }

        // Ten malformed or reserved names must fail closed on all OSes.
        let invalid = [
            "", ".", "..", "../escape", "dir/file",
            "dir\\file", ".git", ".loom", "/absolute", "a/../b",
        ];
        for (case, name) in invalid.iter().enumerate() {
            let node = NativeTree { entries: vec![TreeEntry {
                name: (*name).into(),
                kind: TreeEntryKind::File,
                executable: false,
                object: ObjectId([case as u8; 32]),
            }] };
            assert!(node.encode().is_err(), "invalid case {case}: {name}");
        }
    }

    #[test]
    fn snapshot_rejects_missing_and_wrong_kind_blob_references() {
        use std::collections::BTreeMap;
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        fs::create_dir(&meta).unwrap();
        fs::create_dir(meta.join("objects")).unwrap();
        let store = crate::store::ObjectStore::new(&meta);
        let mut state = BTreeMap::new();
        state.insert("missing.txt".to_string(), ObjectId([7; 32]));
        assert!(write_snapshot(&store, &state).is_err());
        let tree = store.put(TREE, &[]).unwrap();
        state.clear();
        state.insert("wrong-kind.txt".to_string(), tree);
        assert!(write_snapshot(&store, &state).is_err());
    }

    #[test]
    fn flatten_rejects_noncanonical_empty_child_directory() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        fs::create_dir(&meta).unwrap();
        fs::create_dir(meta.join("objects")).unwrap();
        let store = crate::store::ObjectStore::new(&meta);
        let empty = NativeTree { entries: vec![] }.encode().unwrap();
        let (_, payload) = decode_object(&empty).unwrap();
        let child = store.put(TREE, payload).unwrap();
        let root = NativeTree {
            entries: vec![TreeEntry {
                name: "empty".into(),
                kind: TreeEntryKind::Directory,
                executable: false,
                object: child,
            }],
        }
        .encode()
        .unwrap();
        let (_, payload) = decode_object(&root).unwrap();
        let root_id = store.put(TREE, payload).unwrap();
        assert!(flatten_snapshot(&store, root_id).is_err());
    }

    #[test]
    fn incremental_update_rejects_empty_nested_tree() {
        use crate::format::{Change, ChangeKind, BLOB};
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        fs::create_dir(&meta).unwrap();
        fs::create_dir(meta.join("objects")).unwrap();
        let store = crate::store::ObjectStore::new(&meta);
        let empty = NativeTree { entries: vec![] }.encode().unwrap();
        let (_, payload) = decode_object(&empty).unwrap();
        let child = store.put(TREE, payload).unwrap();
        let root = NativeTree {
            entries: vec![TreeEntry {
                name: "dir".into(),
                kind: TreeEntryKind::Directory,
                executable: false,
                object: child,
            }],
        }
        .encode()
        .unwrap();
        let (_, payload) = decode_object(&root).unwrap();
        let root_id = store.put(TREE, payload).unwrap();
        let blob = store.put(BLOB, b"data").unwrap();
        let changes = vec![Change {
            path: "dir/file.txt".into(),
            kind: ChangeKind::Add,
            blob: Some(blob),
        }];
        assert!(apply_changes(&store, root_id, &changes).is_err());
    }

    #[test]
    fn native_tree_rejects_parent_traversal_edits() {
        use crate::format::{Change, ChangeKind, BLOB};
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        fs::create_dir(&meta).unwrap();
        fs::create_dir(meta.join("objects")).unwrap();
        let store = crate::store::ObjectStore::new(&meta);
        let root = write_snapshot(&store, &std::collections::BTreeMap::new()).unwrap();
        let blob = store.put(BLOB, b"safe").unwrap();
        for path in ["../escape", "dir/../escape", "/absolute"] {
            let change = Change {
                path: path.into(),
                kind: ChangeKind::Add,
                blob: Some(blob),
            };
            assert!(apply_changes(&store, root, &[change]).is_err());
        }
    }

    #[test]
    fn roundtrip_and_deterministic_identity() {
        let source = tree();
        let bytes = source.encode().unwrap();
        assert_eq!(bytes[5], crate::format::TREE_VERSION);
        assert_eq!(bytes[6], TREE);
        assert_eq!(NativeTree::decode(&bytes).unwrap(), source);
        assert_eq!(
            crate::format::object_id(&bytes),
            crate::format::object_id(&source.encode().unwrap())
        );
        assert_eq!(NativeTree { entries: vec![] }.encode().unwrap().len(), 19);
    }

    #[test]
    fn empty_tree_matches_independent_canonical_byte_vector() {
        // Golden serialization vector: LOOM-NUL, v2, TREE, 4-byte
        // payload length, then a zero u32 entry count.
        let bytes = NativeTree { entries: vec![] }.encode().unwrap();
        assert_eq!(
            bytes,
            vec![
                0x4c, 0x4f, 0x4f, 0x4d, 0x00, 0x02, 0x03, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00,
            ]
        );
        assert_eq!(NativeTree::decode(&bytes).unwrap().entries.len(), 0);
    }

    #[test]
    fn old_blob_and_changeset_object_versions_remain_separate() {
        let blob = crate::format::encode_object(crate::format::BLOB, b"unchanged").unwrap();
        assert_eq!(blob[5], crate::format::VERSION);
        let tree = NativeTree { entries: vec![] }.encode().unwrap();
        let mut incorrect = blob.clone();
        incorrect[5] = crate::format::TREE_VERSION;
        assert!(crate::format::decode_object(&incorrect).is_err());
        let mut reversed = tree;
        reversed[5] = crate::format::VERSION;
        assert!(NativeTree::decode(&reversed).is_err());
    }

    #[test]
    fn delta_roots_match_canonical_rebuild_across_directory_transitions() {
        use crate::format::{Change, ChangeKind, BLOB};
        use std::collections::BTreeMap;
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        fs::create_dir(&meta).unwrap();
        fs::create_dir(meta.join("objects")).unwrap();
        let store = crate::store::ObjectStore::new(&meta);
        let blob = |contents: &[u8]| store.put(BLOB, contents).unwrap();
        let a = blob(b"a");
        let b = blob(b"b");
        let c = blob(b"c");
        let d = blob(b"d");
        let state = BTreeMap::from([
            ("src/a".to_string(), a),
            ("src/sub/b".to_string(), b),
            ("config".to_string(), c),
        ]);
        let original = write_snapshot(&store, &state).unwrap();
        let original_src = NativeTree::decode(&store.get(original).unwrap())
            .unwrap()
            .entries
            .into_iter()
            .find(|e| e.name == "src")
            .unwrap()
            .object;
        let steps = [
            vec![
                Change {
                    kind: ChangeKind::Modify,
                    path: "src/a".into(),
                    blob: Some(d),
                },
                Change {
                    kind: ChangeKind::Delete,
                    path: "src/sub/b".into(),
                    blob: None,
                },
                Change {
                    kind: ChangeKind::Add,
                    path: "new".into(),
                    blob: Some(b),
                },
            ],
            vec![
                Change {
                    kind: ChangeKind::Delete,
                    path: "config".into(),
                    blob: None,
                },
                Change {
                    kind: ChangeKind::Add,
                    path: "config/nested".into(),
                    blob: Some(c),
                },
            ],
            vec![
                Change {
                    kind: ChangeKind::Delete,
                    path: "config/nested".into(),
                    blob: None,
                },
                Change {
                    kind: ChangeKind::Add,
                    path: "config".into(),
                    blob: Some(a),
                },
            ],
            vec![
                Change {
                    kind: ChangeKind::Delete,
                    path: "src/a".into(),
                    blob: None,
                },
                Change {
                    kind: ChangeKind::Delete,
                    path: "new".into(),
                    blob: None,
                },
                Change {
                    kind: ChangeKind::Delete,
                    path: "config".into(),
                    blob: None,
                },
            ],
        ];
        let mut expected = state;
        let mut root = original;
        for changes in steps {
            for change in &changes {
                if change.kind == ChangeKind::Delete {
                    expected.remove(&change.path);
                } else {
                    expected.insert(change.path.clone(), change.blob.unwrap());
                }
            }
            root = apply_changes(&store, root, &changes).unwrap();
            assert_eq!(root, write_snapshot(&store, &expected).unwrap());
            assert_eq!(flatten_snapshot(&store, root).unwrap(), expected);
        }
        assert!(expected.is_empty());
        // Original root is immutable and its source directory still exists.
        let retained = NativeTree::decode(&store.get(original).unwrap()).unwrap();
        assert_eq!(
            retained
                .entries
                .into_iter()
                .find(|e| e.name == "src")
                .unwrap()
                .object,
            original_src
        );
    }

    #[test]
    fn modifying_native_file_preserves_executable_metadata() {
        use crate::format::{Change, ChangeKind, BLOB};
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        fs::create_dir(&meta).unwrap();
        fs::create_dir(meta.join("objects")).unwrap();
        let store = crate::store::ObjectStore::new(&meta);
        let old_blob = store.put(BLOB, b"old").unwrap();
        let new_blob = store.put(BLOB, b"new").unwrap();
        let root = {
            let tree = NativeTree {
                entries: vec![TreeEntry {
                    name: "tool".into(),
                    kind: TreeEntryKind::File,
                    executable: true,
                    object: old_blob,
                }],
            };
            let envelope = tree.encode().unwrap();
            let (_, payload) = decode_object(&envelope).unwrap();
            store.put(TREE, payload).unwrap()
        };
        let change = Change {
            kind: ChangeKind::Modify,
            path: "tool".into(),
            blob: Some(new_blob),
        };
        let updated = apply_changes(&store, root, std::slice::from_ref(&change)).unwrap();
        let entry = NativeTree::decode(&store.get(updated).unwrap())
            .unwrap()
            .entries
            .remove(0);
        assert!(entry.executable);
        assert_eq!(entry.object, new_blob);
        assert_eq!(apply_changes(&store, root, &[change]).unwrap(), updated);
        let original = NativeTree::decode(&store.get(root).unwrap())
            .unwrap()
            .entries
            .remove(0);
        assert!(original.executable);
        assert_eq!(original.object, old_blob);
    }

    #[test]
    fn unchanged_sibling_trees_keep_their_original_ids() {
        use crate::format::{Change, ChangeKind, BLOB};
        use std::collections::BTreeMap;
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        fs::create_dir(&meta).unwrap();
        fs::create_dir(meta.join("objects")).unwrap();
        let store = crate::store::ObjectStore::new(&meta);
        let a = store.put(BLOB, b"a").unwrap();
        let b = store.put(BLOB, b"b").unwrap();
        let c = store.put(BLOB, b"c").unwrap();
        let old = write_snapshot(
            &store,
            &BTreeMap::from([("left/a".to_string(), a), ("right/b".to_string(), b)]),
        )
        .unwrap();
        let next = apply_changes(
            &store,
            old,
            &[Change {
                kind: ChangeKind::Modify,
                path: "left/a".into(),
                blob: Some(c),
            }],
        )
        .unwrap();
        let child = |root| {
            NativeTree::decode(&store.get(root).unwrap())
                .unwrap()
                .entries
                .into_iter()
                .find(|e| e.name == "right")
                .unwrap()
                .object
        };
        assert_eq!(child(old), child(next));
        assert_ne!(old, next);
        assert_eq!(apply_changes(&store, old, &[]).unwrap(), old);
    }

    #[test]
    fn invalid_edits_fail_without_publishing_a_new_root() {
        use crate::format::{Change, ChangeKind, BLOB};
        use std::collections::BTreeMap;
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        fs::create_dir(&meta).unwrap();
        fs::create_dir(meta.join("objects")).unwrap();
        let store = crate::store::ObjectStore::new(&meta);
        let a = store.put(BLOB, b"a").unwrap();
        let root = write_snapshot(&store, &BTreeMap::from([("file".to_string(), a)])).unwrap();
        let add = |path: &str| Change {
            kind: ChangeKind::Add,
            path: path.into(),
            blob: Some(a),
        };
        assert!(apply_changes(&store, root, &[add("file/nested")]).is_err());
        let nested = write_snapshot(
            &store,
            &BTreeMap::from([("dir/a".to_string(), a), ("dir/b".to_string(), a)]),
        )
        .unwrap();
        assert!(apply_changes(&store, nested, &[add("dir")]).is_err());
        assert!(apply_changes(
            &store,
            nested,
            &[Change {
                kind: ChangeKind::Delete,
                path: "dir".into(),
                blob: None
            }]
        )
        .is_err());
        assert!(
            apply_changes(
                &store,
                nested,
                &[
                    add("dir"),
                    Change {
                        kind: ChangeKind::Delete,
                        path: "dir/a".into(),
                        blob: None
                    }
                ]
            )
            .is_err(),
            "other directory descendants must not disappear silently"
        );
        assert!(apply_changes(&store, root, &[add("x"), add("x/y")]).is_err());
        assert!(apply_changes(&store, root, &[add("../escape")]).is_err());
        assert!(apply_changes(
            &store,
            root,
            &[Change {
                kind: ChangeKind::Delete,
                path: "file".into(),
                blob: Some(a)
            }]
        )
        .is_err());
        assert!(apply_changes(&store, root, &[add("x"), add("x")]).is_err());
        assert!(apply_changes(
            &store,
            root,
            &[Change {
                kind: ChangeKind::Add,
                path: "blob".into(),
                blob: Some(root)
            }]
        )
        .is_err());
        assert_eq!(
            flatten_snapshot(&store, root).unwrap().get("file"),
            Some(&a)
        );
    }

    #[test]
    fn reject_noncanonical_names_and_directory_flags() {
        let cases = ["../escape", "two/components", ".loom", ".git", "a\\b", ""];
        for name in cases {
            let mut value = tree();
            value.entries[0].name = name.into();
            assert!(value.encode().is_err(), "{name}");
        }
        let mut duplicate = tree();
        duplicate.entries.push(duplicate.entries[1].clone());
        assert!(duplicate.encode().is_err());
        let mut wrong_order = tree();
        wrong_order.entries.reverse();
        assert!(wrong_order.encode().is_err());
        let mut flag = tree();
        flag.entries[1].executable = true;
        assert!(flag.encode().is_err());
    }

    #[test]
    fn reject_noncanonical_serialized_data() {
        let bytes = tree().encode().unwrap();
        for len in [0, 4, 15, bytes.len() - 1] {
            assert!(NativeTree::decode(&bytes[..len]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(42);
        assert!(NativeTree::decode(&trailing).is_err());
        let mut invalid_flag = bytes.clone();
        invalid_flag[15 + 4 + 1] = 3;
        assert!(NativeTree::decode(&invalid_flag).is_err());
        let mut wrong_kind = bytes.clone();
        wrong_kind[6] = crate::format::BLOB;
        assert!(NativeTree::decode(&wrong_kind).is_err());
    }
    #[test]
    fn parallel_independent_subtrees_match_serial_canonical_root() {
        use crate::format::{Change, ChangeKind, BLOB};
        use std::collections::BTreeMap;
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join(".loom");
        fs::create_dir(&meta).unwrap();
        fs::create_dir(meta.join("objects")).unwrap();
        let store = crate::store::ObjectStore::new(&meta);
        let before = store.put(BLOB, b"before").unwrap();
        let after = store.put(BLOB, b"after").unwrap();
        let mut state = BTreeMap::new();
        let mut changes = Vec::new();
        for index in 0..64 {
            let path = format!("root-{index:02}/nested/file");
            state.insert(path.clone(), before);
            changes.push(Change {
                kind: ChangeKind::Modify,
                path,
                blob: Some(after),
            });
        }
        state.insert("untouched/file".into(), before);
        let root = write_snapshot(&store, &state).unwrap();

        let serial = apply_changes_with_workers(&store, root, &changes, 1).unwrap();
        let parallel = apply_changes_with_workers(&store, root, &changes, 8).unwrap();
        assert_eq!(parallel, serial);
        assert_eq!(flatten_snapshot(&store, parallel).unwrap().len(), 65);
        assert_eq!(
            lookup_file(&store, parallel, "untouched/file").unwrap(),
            Some(before)
        );
        for change in changes {
            assert_eq!(
                lookup_file(&store, parallel, &change.path).unwrap(),
                Some(after)
            );
        }
    }

    #[test]
    fn nested_subtrees_overlap_without_exceeding_provider_or_nested_worker_limits() {
        use crate::format::{Change, ChangeKind, BLOB};
        use std::collections::BTreeMap;
        for limit in [1, 2] {
            let dir = tempfile::tempdir().unwrap();
            let backend = Arc::new(PublicationProbe::new(dir.path(), limit));
            let store = crate::store::ObjectStore::with_backend(backend.clone()).unwrap();
            let before = store.put(BLOB, b"before").unwrap();
            let after = store.put(BLOB, b"after").unwrap();
            let mut state = BTreeMap::new();
            let mut changes = Vec::new();
            for index in 0..64 {
                let path = format!("shared/prefix/branch-{}/deep-{index:02}/file", index % 4);
                state.insert(path.clone(), before);
                changes.push(Change {
                    path,
                    kind: ChangeKind::Modify,
                    blob: Some(after),
                });
            }
            // Cheap siblings must not hide the independent directory tasks.
            for index in 0..32 {
                let path = format!("shared/prefix/top-{index:02}");
                state.insert(path.clone(), before);
                changes.push(Change {
                    path,
                    kind: ChangeKind::Modify,
                    blob: Some(after),
                });
            }
            state.insert("untouched/file".into(), before);
            let root = write_snapshot(&store, &state).unwrap();
            let serial = apply_changes_with_workers(&store, root, &changes, 1).unwrap();
            backend.observe.store(true, Ordering::SeqCst);
            let parallel = apply_changes_with_workers(&store, root, &changes, 8).unwrap();
            assert_eq!(parallel, serial);
            assert_eq!(backend.active.load(Ordering::SeqCst), 0);
            let expected = limit.min(std::thread::available_parallelism().map_or(1, usize::from));
            assert_eq!(backend.peak.load(Ordering::SeqCst), expected);
            for change in changes {
                state.insert(change.path, after);
            }
            assert_eq!(flatten_snapshot(&store, parallel).unwrap(), state);
        }
    }

    #[test]
    fn nested_tree_failure_preserves_ref_and_staged_snapshot_for_retry() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let backend = Arc::new(PublicationProbe::new(&temp.path().join("objects"), 2));
        let repo =
            crate::Repository::init_with_object_backend(&workspace, backend.clone()).unwrap();
        for name in ["left", "right"] {
            let path = workspace.join(format!("shared/prefix/{name}/file"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"before").unwrap();
        }
        let before = repo.commit("test", "base").unwrap();
        for name in ["left", "right"] {
            std::fs::write(
                workspace.join(format!("shared/prefix/{name}/file")),
                b"after",
            )
            .unwrap();
        }
        repo.add("shared").unwrap();
        let staged = repo.diff_staged().unwrap();
        backend.observe.store(true, Ordering::SeqCst);
        backend.fail.store(true, Ordering::SeqCst);
        assert!(repo
            .commit("test", "reject failed native-tree write")
            .is_err());
        assert_eq!(repo.head().unwrap(), Some(before));
        assert_eq!(repo.diff_staged().unwrap(), staged);
        assert_eq!(backend.active.load(Ordering::SeqCst), 0);
        backend.fail.store(false, Ordering::SeqCst);
        let after = repo.commit("test", "retry native-tree write").unwrap();
        assert_ne!(before, after);
        for name in ["left", "right"] {
            assert_eq!(
                repo.read_branch_file("main", &format!("shared/prefix/{name}/file"))
                    .unwrap()
                    .unwrap(),
                b"after"
            );
        }
        repo.fsck().unwrap();
    }
}
