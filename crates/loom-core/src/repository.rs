use crate::format::{decode_object, encode_object, object_id, validate_path, Change, ChangeKind, Changeset, ObjectId, BLOB, CHANGESET, MAX_OBJECT_SIZE};
use crate::store::ObjectStore;
use crate::{Error, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind { Added, Modified, Deleted }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry { pub path: String, pub kind: StatusKind }

pub struct Repository { root: PathBuf, db: Connection, store: ObjectStore }

impl Repository {
    pub fn init(path: impl AsRef<Path>) -> Result<Self> {
        let root = fs::canonicalize(path)?;
        let meta = root.join(".loom");
        if meta.exists() { return Err(Error::Invalid("already initialized".into())); }
        fs::create_dir(&meta)?;
        fs::create_dir(meta.join("objects"))?;
        let db = Self::connect(&meta)?;
        db.execute_batch(
            "CREATE TABLE refs (name TEXT PRIMARY KEY NOT NULL, oid TEXT);
             CREATE TABLE meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);
             INSERT INTO refs(name,oid) VALUES ('main',NULL);
             INSERT INTO meta(key,value) VALUES ('head','main');"
        )?;
        Ok(Self { root, db, store: ObjectStore::new(&meta) })
    }

    pub fn discover(path: impl AsRef<Path>) -> Result<Self> {
        let start = fs::canonicalize(path)?;
        for dir in start.ancestors() {
            let meta = dir.join(".loom");
            if meta.join("index.sqlite").exists() {
                return Ok(Self { root: dir.to_path_buf(), db: Self::connect(&meta)?, store: ObjectStore::new(&meta) });
            }
        }
        Err(Error::Invalid("not in a Loom repository".into()))
    }

    fn connect(meta: &Path) -> Result<Connection> {
        let db = Connection::open(meta.join("index.sqlite"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA busy_timeout=5000;")?;
        Ok(db)
    }

    pub fn root(&self) -> &Path { &self.root }
    pub fn current_branch(&self) -> Result<String> {
        self.db.query_row("SELECT value FROM meta WHERE key='head'", [], |r| r.get(0)).map_err(Into::into)
    }
    pub fn head(&self) -> Result<Option<ObjectId>> {
        self.branch_tip(&self.current_branch()?)
    }
    pub fn branch_tip(&self, name: &str) -> Result<Option<ObjectId>> {
        let oid: Option<Option<String>> = self.db.query_row(
            "SELECT oid FROM refs WHERE name=?1", [name], |r| r.get(0)).optional()?;
        match oid {
            None => Err(Error::Invalid(format!("unknown branch {name}"))),
            Some(None) => Ok(None),
            Some(Some(s)) => Ok(Some(ObjectId::parse(&s)?)),
        }
    }
    pub fn branches(&self) -> Result<Vec<(String, Option<ObjectId>)>> {
        let mut stmt = self.db.prepare("SELECT name,oid FROM refs ORDER BY name")?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            let name: String = r.get(0)?;
            let id: Option<String> = r.get(1)?;
            out.push((name, id.map(|s| ObjectId::parse(&s)).transpose()?));
        }
        Ok(out)
    }
    fn save_ref_cas(&self, name: &str, expected: Option<ObjectId>, next: ObjectId) -> Result<()> {
        // SQLite IS safely supports null and values, and protects against lost updates.
        let changed = self.db.execute("UPDATE refs SET oid=?1 WHERE name=?2 AND oid IS ?3",
            params![next.to_string(), name, expected.map(|id| id.to_string())])?;
        if changed != 1 { return Err(Error::Conflict(format!("branch {name} moved; retry after inspecting current head"))); }
        Ok(())
    }
    pub fn create_branch(&self, name: &str) -> Result<()> {
        if name.is_empty() || name.len() > 128 ||
           !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.')) ||
           name.starts_with('/') || name.ends_with('/') || name.contains("..") || name.contains("//") {
            return Err(Error::Invalid("invalid branch name".into()));
        }
        self.db.execute("INSERT INTO refs(name,oid) VALUES(?1,?2)",
            params![name, self.head()?.map(|id| id.to_string())])?;
        Ok(())
    }

    fn get_changeset(&self, id: ObjectId) -> Result<Changeset> {
        let raw = self.store.get(id)?;
        Changeset::decode(&raw)
    }
    fn state_at(&self, tip: Option<ObjectId>) -> Result<BTreeMap<String,ObjectId>> {
        let mut commits = Vec::new();
        let mut visited = HashSet::new();
        let mut next = tip;
        while let Some(id) = next {
            if !visited.insert(id) { return Err(Error::Invalid("cycle in first-parent history".into())); }
            if commits.len() > 1_000_000 { return Err(Error::Invalid("history depth limit reached".into())); }
            let c = self.get_changeset(id)?;
            next = c.parents.first().copied();
            commits.push(c);
        }
        let mut state = BTreeMap::new();
        for c in commits.iter().rev() {
            for ch in &c.changes {
                if ch.kind == ChangeKind::Delete { state.remove(&ch.path); }
                else { state.insert(ch.path.clone(), ch.blob.ok_or_else(|| Error::Invalid("missing blob in history".into()))?); }
            }
        }
        Ok(state)
    }

    fn scan_files(&self) -> Result<BTreeMap<String, Vec<u8>>> {
        let mut result = BTreeMap::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            for item in fs::read_dir(&dir)? {
                let item = item?;
                let kind = item.file_type()?;
                let name = item.file_name();
                if name == ".loom" || name == ".git" { continue; }
                if kind.is_symlink() { return Err(Error::Invalid(format!("symlinks unsupported in v1: {:?}", item.path()))); }
                if kind.is_dir() { stack.push(item.path()); continue; }
                if !kind.is_file() { return Err(Error::Invalid(format!("unsupported file type: {:?}", item.path()))); }
                let relative = item.path().strip_prefix(&self.root).map_err(|_| Error::Invalid("path escaped worktree".into()))?.to_path_buf();
                let s = relative.to_str().ok_or_else(|| Error::Invalid("non-UTF-8 path not supported in v1".into()))?;
                let key = if cfg!(windows) { s.replace('\\', "/") } else { s.to_string() };
                validate_path(&key)?;
                let metadata = item.metadata()?;
                if metadata.len() > MAX_OBJECT_SIZE as u64 { return Err(Error::Invalid(format!("file exceeds 64 MiB: {key}"))); }
                result.insert(key, fs::read(item.path())?);
            }
        }
        Ok(result)
    }

    fn make_delta(&self, old: &BTreeMap<String,ObjectId>, files: &BTreeMap<String, Vec<u8>>, persist: bool) -> Result<Vec<Change>> {
        let mut changes = Vec::new();
        for (path, bytes) in files {
            let data = encode_object(BLOB, bytes)?;
            let id = object_id(&data);
            let prior = old.get(path);
            if prior != Some(&id) {
                if persist { self.store.put(BLOB, bytes)?; }
                changes.push(Change { kind: if prior.is_some() { ChangeKind::Modify } else { ChangeKind::Add }, path: path.clone(), blob: Some(id) });
            }
        }
        for path in old.keys() {
            if !files.contains_key(path) {
                changes.push(Change { kind: ChangeKind::Delete, path: path.clone(), blob: None });
            }
        }
        changes.sort_by(|a,b| a.path.cmp(&b.path));
        Ok(changes)
    }

    pub fn status(&self) -> Result<Vec<StatusEntry>> {
        let old = self.state_at(self.head()?)?;
        let files = self.scan_files()?;
        let changes = self.make_delta(&old, &files, false)?;
        Ok(changes.into_iter().map(|ch| StatusEntry { path: ch.path, kind: match ch.kind {
            ChangeKind::Add => StatusKind::Added, ChangeKind::Modify => StatusKind::Modified,
            ChangeKind::Delete => StatusKind::Deleted
        }}).collect())
    }

    pub fn commit(&self, author: &str, message: &str) -> Result<ObjectId> {
        if author.trim().is_empty() || message.trim().is_empty() { return Err(Error::Invalid("author and commit message are required".into())); }
        let branch = self.current_branch()?;
        let prev = self.branch_tip(&branch)?;
        let old = self.state_at(prev)?;
        let files = self.scan_files()?;
        let changes = self.make_delta(&old, &files, true)?;
        if changes.is_empty() { return Err(Error::Invalid("nothing to commit".into())); }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| Error::Invalid("system time before epoch".into()))?;
        let c = Changeset { parents: prev.into_iter().collect(), author: author.into(), message: message.into(),
            timestamp_seconds: now.as_secs() as i64, changes };
        let raw = c.encode()?;
        let (_, payload) = decode_object(&raw)?;
        let id = self.store.put(CHANGESET, payload)?;
        self.save_ref_cas(&branch, prev, id)?;
        Ok(id)
    }

    pub fn log(&self, limit: usize) -> Result<Vec<(ObjectId, Changeset)>> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut next = self.head()?;
        while let Some(id) = next {
            if result.len() >= limit.min(10_000) { break; }
            if !visited.insert(id) { return Err(Error::Invalid("history cycle".into())); }
            let c = self.get_changeset(id)?;
            next = c.parents.first().copied();
            result.push((id, c));
        }
        Ok(result)
    }

    fn materialize(&self, target: &BTreeMap<String,ObjectId>, previous: &BTreeMap<String,ObjectId>) -> Result<()> {
        // No untracked files are permitted by switch's clean-working-tree gate.
        for path in previous.keys() {
            if !target.contains_key(path) {
                let full = self.root.join(path);
                fs::remove_file(full)?;
            }
        }
        // Remove now-empty directories deepest first to permit file/directory transitions.
        let mut dirs: Vec<PathBuf> = previous.keys().filter_map(|p| self.root.join(p).parent().map(Path::to_path_buf)).collect();
        dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        dirs.dedup();
        for dir in dirs {
            if dir != self.root { let _ = fs::remove_dir(dir); }
        }
        for (path, oid) in target {
            if previous.get(path) == Some(oid) { continue; }
            let raw = self.store.get(*oid)?;
            let (kind, bytes) = decode_object(&raw)?;
            if kind != BLOB { return Err(Error::Invalid(format!("non-blob at {path}"))); }
            let full = self.root.join(path);
            if let Some(parent) = full.parent() { fs::create_dir_all(parent)?; }
            fs::write(full, bytes)?;
        }
        Ok(())
    }
    pub fn switch_branch(&self, name: &str) -> Result<()> {
        if !self.status()?.is_empty() { return Err(Error::Conflict("working tree has uncommitted or untracked files".into())); }
        let old = self.state_at(self.head()?)?;
        let new = self.state_at(self.branch_tip(name)?)?;
        for id in new.values() { self.store.get(*id)?; }
        self.materialize(&new, &old)?;
        self.db.execute("UPDATE meta SET value=?1 WHERE key='head'", [name])?;
        Ok(())
    }


    fn ancestor_distances(&self, start: ObjectId) -> Result<HashMap<ObjectId, usize>> {
        let mut distances = HashMap::new();
        let mut queue = VecDeque::from([(start, 0usize)]);
        while let Some((id, distance)) = queue.pop_front() {
            if distances.contains_key(&id) { continue; }
            if distances.len() >= 1_000_000 { return Err(Error::Invalid("ancestry traversal limit reached".into())); }
            distances.insert(id, distance);
            let commit = self.get_changeset(id)?;
            for parent in commit.parents {
                queue.push_back((parent, distance + 1));
            }
        }
        Ok(distances)
    }

    fn merge_base(&self, ours: ObjectId, theirs: ObjectId) -> Result<Option<ObjectId>> {
        let left = self.ancestor_distances(ours)?;
        let right = self.ancestor_distances(theirs)?;
        Ok(left.iter().filter_map(|(id, l)| right.get(id).map(|r| (*id, l+r)))
            .min_by(|a,b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0))).map(|(id,_)| id))
    }

    /// Conservative merge: non-overlapping file edits merge automatically.
    /// Ambiguous edits are rejected without creating a merge commit or changing the worktree.
    /// This is not yet a line-level three-way merge.
    pub fn merge_branch(&self, name: &str, author: &str, message: &str) -> Result<ObjectId> {
        if author.trim().is_empty() || message.trim().is_empty() {
            return Err(Error::Invalid("author and message required".into()));
        }
        if !self.status()?.is_empty() {
            return Err(Error::Conflict("working tree must be clean before merge".into()));
        }
        let branch = self.current_branch()?;
        let ours = self.head()?.ok_or_else(|| Error::Invalid("cannot merge into unborn branch".into()))?;
        let theirs = self.branch_tip(name)?.ok_or_else(|| Error::Invalid("cannot merge unborn branch".into()))?;
        if ours == theirs { return Ok(ours); }
        let base = self.state_at(self.merge_base(ours, theirs)?)?;
        let our_files = self.state_at(Some(ours))?;
        let their_files = self.state_at(Some(theirs))?;
        let mut keys = BTreeSet::new();
        keys.extend(base.keys().cloned());
        keys.extend(our_files.keys().cloned());
        keys.extend(their_files.keys().cloned());
        let mut merged = BTreeMap::new();
        let mut conflicts = Vec::new();
        for path in keys {
            let original = base.get(&path);
            let local = our_files.get(&path);
            let remote = their_files.get(&path);
            let chosen = if local == remote { local }
                else if local == original { remote }
                else if remote == original { local }
                else { conflicts.push(path.clone()); continue };
            if let Some(id) = chosen { merged.insert(path, *id); }
        }
        // Reject file/directory collisions before publishing the merge commit.
        for path in merged.keys() {
            for (idx, chr) in path.char_indices() {
                if chr == '/' && merged.contains_key(&path[..idx]) {
                    conflicts.push(path.clone());
                }
            }
        }
        if !conflicts.is_empty() {
            conflicts.sort();
            conflicts.dedup();
            return Err(Error::Conflict(format!("merge conflicts in: {}", conflicts.join(", "))));
        }
        let mut changes = Vec::new();
        for (path, id) in &merged {
            if our_files.get(path) != Some(id) {
                changes.push(Change { path: path.clone(), kind: if our_files.contains_key(path) {
                    ChangeKind::Modify
                } else { ChangeKind::Add }, blob: Some(*id) });
            }
        }
        for path in our_files.keys() {
            if !merged.contains_key(path) {
                changes.push(Change { path: path.clone(), kind: ChangeKind::Delete, blob: None });
            }
        }
        changes.sort_by(|a,b| a.path.cmp(&b.path));
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| Error::Invalid("system time before epoch".into()))?;
        let commit = Changeset { parents: vec![ours, theirs], author: author.into(), message: message.into(),
            timestamp_seconds: now.as_secs() as i64, changes };
        let encoded = commit.encode()?;
        let (_, payload) = decode_object(&encoded)?;
        let id = self.store.put(CHANGESET, payload)?;
        self.save_ref_cas(&branch, Some(ours), id)?;
        // Workspace materialization is not crash-atomic in v1.
        self.materialize(&merged, &our_files)?;
        Ok(id)
    }

    pub fn fsck(&self) -> Result<usize> {
        let ids = self.store.ids()?;
        let mut checked = 0usize;
        for id in &ids {
            let data = self.store.get(*id)?;
            let (kind, _) = decode_object(&data)?;
            if kind == CHANGESET {
                let c = Changeset::decode(&data)?;
                for parent in c.parents {
                    let raw = self.store.get(parent)?;
                    if decode_object(&raw)?.0 != CHANGESET { return Err(Error::Invalid(format!("non-changeset parent: {parent}"))); }
                }
                for ch in c.changes {
                    if let Some(blob) = ch.blob {
                        let raw = self.store.get(blob)?;
                        if decode_object(&raw)?.0 != BLOB { return Err(Error::Invalid(format!("non-blob referenced at {}", ch.path))); }
                    }
                }
            }
            checked += 1;
        }
        for (_, tip) in self.branches()? {
            if let Some(id) = tip { self.get_changeset(id)?; self.state_at(Some(id))?; }
        }
        Ok(checked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repo() -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        (dir, repo)
    }
    #[test] fn basic_commit_status_log_fsck() {
        let (dir, r) = repo();
        assert_eq!(r.current_branch().unwrap(), "main");
        fs::write(dir.path().join("hello.txt"), b"hello").unwrap();
        assert_eq!(r.status().unwrap()[0].kind, StatusKind::Added);
        let first = r.commit("tester", "initial").unwrap();
        assert!(r.status().unwrap().is_empty());
        fs::write(dir.path().join("hello.txt"), b"world").unwrap();
        let second = r.commit("tester", "update").unwrap();
        assert_eq!(r.log(10).unwrap().len(), 2);
        assert_eq!(r.get_changeset(second).unwrap().parents, vec![first]);
        assert!(r.fsck().unwrap() >= 4);
    }
    #[test] fn branches_switch_and_cas() {
        let (dir, r) = repo();
        fs::write(dir.path().join("a"), b"first").unwrap();
        r.commit("a", "c1").unwrap();
        r.create_branch("feature").unwrap();
        r.switch_branch("feature").unwrap();
        fs::write(dir.path().join("a"), b"second").unwrap();
        r.commit("a", "c2").unwrap();
        r.switch_branch("main").unwrap();
        assert_eq!(fs::read(dir.path().join("a")).unwrap(), b"first");
        assert_eq!(r.current_branch().unwrap(), "main");
    }
    #[test] fn corrupt_object_detected() {
        let (_, r) = repo();
        let oid = r.store.put(BLOB, b"x").unwrap();
        fs::write(r.store.path_for_test(oid), b"corrupt").unwrap();
        assert!(r.fsck().is_err());
    }
    #[test] fn non_overlapping_merge_and_conflict_rejection() {
        let (dir, r) = repo();
        fs::write(dir.path().join("a"), b"base-a").unwrap();
        fs::write(dir.path().join("b"), b"base-b").unwrap();
        r.commit("dev", "base").unwrap();
        r.create_branch("feature").unwrap();
        r.switch_branch("feature").unwrap();
        fs::write(dir.path().join("b"), b"feature-b").unwrap();
        r.commit("dev", "feature").unwrap();
        r.switch_branch("main").unwrap();
        fs::write(dir.path().join("a"), b"main-a").unwrap();
        r.commit("dev", "main").unwrap();
        let merge = r.merge_branch("feature", "dev", "merge").unwrap();
        assert_eq!(r.get_changeset(merge).unwrap().parents.len(), 2);
        assert_eq!(fs::read(dir.path().join("a")).unwrap(), b"main-a");
        assert_eq!(fs::read(dir.path().join("b")).unwrap(), b"feature-b");
        assert!(r.status().unwrap().is_empty());
        assert!(r.fsck().is_ok());
        r.create_branch("conflict").unwrap();
        r.switch_branch("conflict").unwrap();
        fs::write(dir.path().join("a"), b"conflict-side").unwrap();
        r.commit("dev", "conflicting").unwrap();
        r.switch_branch("main").unwrap();
        fs::write(dir.path().join("a"), b"another-side").unwrap();
        r.commit("dev", "another").unwrap();
        let previous = r.head().unwrap();
        assert!(r.merge_branch("conflict", "dev", "nope").is_err());
        assert_eq!(r.head().unwrap(), previous);
        assert_eq!(fs::read(dir.path().join("a")).unwrap(), b"another-side");
    }

}
