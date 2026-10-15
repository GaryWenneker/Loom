use crate::format::{decode_object, encode_object, object_id, validate_path, Change, ChangeKind, Changeset, ObjectId, BLOB, CHANGESET, MAX_OBJECT_SIZE};
use crate::store::ObjectStore;
use crate::{Error, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind { Added, Modified, Deleted }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry { pub path: String, pub kind: StatusKind }

/// File contents before and after a change. None means the file is absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff { pub path: String, pub before: Option<Vec<u8>>, pub after: Option<Vec<u8>> }

static STAGE_NONCE: AtomicU64 = AtomicU64::new(0);

// A durable transition descriptor. Object IDs point to immutable snapshots; SQLite
// publishes the ref/HEAD change only after worktree materialization completes.
#[derive(Debug, Clone)]
struct PendingTransition {
    operation: String,
    from_branch: String,
    to_branch: String,
    before: Option<ObjectId>,
    after: Option<ObjectId>,
}

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
                let repo = Self { root: dir.to_path_buf(), db: Self::connect(&meta)?, store: ObjectStore::new(&meta) };
                {
                    let _guard = repo.lock_worktree()?;
                    repo.recover_locked()?;
                }
                return Ok(repo);
            }
        }
        Err(Error::Invalid("not in a Loom repository".into()))
    }

    fn connect(meta: &Path) -> Result<Connection> {
        let db = Connection::open(meta.join("index.sqlite"))?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS stage_index (
                 path TEXT PRIMARY KEY NOT NULL,
                 blob_id TEXT
             );
             CREATE TABLE IF NOT EXISTS branch_path_index (
                 branch TEXT NOT NULL,
                 path TEXT NOT NULL,
                 oid TEXT NOT NULL,
                 PRIMARY KEY (branch, path)
             );
             CREATE TABLE IF NOT EXISTS branch_path_index_tip (
                 branch TEXT PRIMARY KEY NOT NULL,
                 oid TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS workspace_journal (
                 id INTEGER PRIMARY KEY CHECK(id=1),
                 operation TEXT NOT NULL,
                 from_branch TEXT NOT NULL,
                 to_branch TEXT NOT NULL,
                 before_oid TEXT,
                 after_oid TEXT
             );"
        )?;
        Ok(db)
    }

    /// Exclusive advisory lock shared by all Loom CLI processes using this worktree.
    /// The OS releases this file lock on process exit, including abnormal termination.
    fn lock_worktree(&self) -> Result<fs::File> {
        use fs2::FileExt;
        let lock_path = self.root.join(".loom").join("workspace.lock");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(lock_path)?;
        file.lock_exclusive()?;
        Ok(file)
    }


    fn pending_transition(&self) -> Result<Option<PendingTransition>> {
        type JournalRow = (String, String, String, Option<String>, Option<String>);
        let row: Option<JournalRow> = self.db.query_row(
            "SELECT operation,from_branch,to_branch,before_oid,after_oid
             FROM workspace_journal WHERE id=1",
            [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))
        ).optional()?;
        row.map(|(operation,from_branch,to_branch,before,after)| {
            Ok(PendingTransition {
                operation, from_branch, to_branch,
                before: before.map(|s| ObjectId::parse(&s)).transpose()?,
                after: after.map(|s| ObjectId::parse(&s)).transpose()?,
            })
        }).transpose()
    }

    fn begin_transition(&self, p: &PendingTransition) -> Result<()> {
        // The INSERT is a durable intent record before the first filesystem change.
        self.db.execute(
            "INSERT INTO workspace_journal
             (id,operation,from_branch,to_branch,before_oid,after_oid)
             VALUES (1,?1,?2,?3,?4,?5)",
            params![p.operation,p.from_branch,p.to_branch,
                p.before.map(|id| id.to_string()),p.after.map(|id| id.to_string())]
        )?;
        Ok(())
    }

    /// Replay an interrupted transition while holding the cross-process lock.
    /// Refuse to overwrite unexpected user edits or unexpected branch movement.
    fn recover_locked(&self) -> Result<()> {
        let Some(p) = self.pending_transition()? else { return Ok(()); };
        if self.current_branch()? != p.from_branch {
            return Err(Error::Conflict("workspace recovery requires the original HEAD branch".into()));
        }
        if self.branch_tip(&p.from_branch)? != p.before {
            return Err(Error::Conflict("workspace recovery halted: original ref changed".into()));
        }
        match p.operation.as_str() {
            "switch" => {
                if self.branch_tip(&p.to_branch)? != p.after {
                    return Err(Error::Conflict("workspace recovery halted: destination ref changed".into()));
                }
            }
            "merge" => {
                if p.to_branch != p.from_branch || p.after.is_none() {
                    return Err(Error::Invalid("malformed merge recovery journal".into()));
                }
                let target = self.get_changeset(p.after.expect("checked above"))?;
                if target.parents.first().copied() != p.before {
                    return Err(Error::Invalid("merge journal parent mismatch".into()));
                }
            }
            _ => return Err(Error::Invalid("unknown workspace journal operation".into())),
        }
        let before = self.state_at(p.before)?;
        let after = self.state_at(p.after)?;
        self.materialize(&after, &before)?;
        // The only commit point: both HEAD/ref update and journal deletion are atomic.
        let tx = self.db.unchecked_transaction()?;
        if p.operation == "switch" {
            let changed = tx.execute(
                "UPDATE meta SET value=?1 WHERE key='head' AND value=?2",
                params![p.to_branch,p.from_branch]
            )?;
            if changed != 1 { return Err(Error::Conflict("HEAD changed during recovery".into())); }
        } else {
            let changed = tx.execute(
                "UPDATE refs SET oid=?1 WHERE name=?2 AND oid IS ?3",
                params![p.after.map(|id| id.to_string()),p.from_branch,
                    p.before.map(|id| id.to_string())]
            )?;
            if changed != 1 { return Err(Error::Conflict("merge ref moved during recovery".into())); }
        }
        tx.execute("DELETE FROM workspace_journal WHERE id=1", [])?;
        tx.commit()?;
        // Staged files never live in the user worktree. A crash before cleanup is harmless.
        let _ = fs::remove_dir_all(self.root.join(".loom").join("staging"));
        Ok(())
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
        let _worktree_guard = self.lock_worktree()?;
        self.recover_locked()?;
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

    /// Read a single user-selected path, without scanning unrelated files.
    /// Missing or directory paths are represented as None for staged deletion.
    /// This is an alpha safeguard, not a race-free handle-relative traversal.
    fn read_staging_path(&self, path: &str) -> Result<Option<Vec<u8>>> {
        validate_path(path)?;
        let mut ancestor = self.root.clone();
        let mut parts = path.split('/').peekable();
        while let Some(component) = parts.next() {
            ancestor.push(component);
            let metadata = match fs::symlink_metadata(&ancestor) {
                Ok(value) => value,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            if metadata.file_type().is_symlink() {
                return Err(Error::Invalid(format!(
                    "symlinks unsupported in staged path: {path}"
                )));
            }
            if parts.peek().is_some() {
                if !metadata.is_dir() {
                    return Ok(None);
                }
                continue;
            }
            if metadata.is_dir() { return Ok(None); }
            if !metadata.is_file() {
                return Err(Error::Invalid(format!("unsupported staged file type: {path}")));
            }
            if metadata.len() > MAX_OBJECT_SIZE as u64 {
                return Err(Error::Invalid(format!("file exceeds 64 MiB: {path}")));
            }
            use std::io::Read;
            let file = fs::File::open(&ancestor)?;
            if !file.metadata()?.is_file() {
                return Err(Error::Invalid(format!("staged path changed type: {path}")));
            }
            let mut bytes = Vec::new();
            file.take((MAX_OBJECT_SIZE + 1) as u64).read_to_end(&mut bytes)?;
            if bytes.len() > MAX_OBJECT_SIZE {
                return Err(Error::Invalid(format!("file exceeds 64 MiB: {path}")));
            }
            return Ok(Some(bytes));
        }
        Ok(None)
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

    /// Staging is opt-in for existing v1 repositories. Once enabled, commits
    /// always require an explicit set of staged paths; there is no silent fallback.
    pub fn staging_enabled(&self) -> Result<bool> {
        let value: Option<String> = self.db.query_row(
            "SELECT value FROM meta WHERE key='staging_mode'", [], |r| r.get(0)
        ).optional()?;
        Ok(value.as_deref() == Some("enabled"))
    }

    fn staged_blobs(&self) -> Result<BTreeMap<String, Option<ObjectId>>> {
        let mut rows = self.db.prepare("SELECT path,blob_id FROM stage_index ORDER BY path")?;
        let mut entries = rows.query([])?;
        let mut map = BTreeMap::new();
        while let Some(row) = entries.next()? {
            let path: String = row.get(0)?;
            validate_path(&path)?;
            let oid: Option<String> = row.get(1)?;
            map.insert(path, oid.map(|value| ObjectId::parse(&value)).transpose()?);
        }
        Ok(map)
    }

    pub fn staged_status(&self) -> Result<Vec<StatusEntry>> {
        let old = self.state_at(self.head()?)?;
        let mut changes = Vec::new();
        for (path, blob) in self.staged_blobs()? {
            let kind = match (old.get(&path), blob) {
                (None, Some(_)) => StatusKind::Added,
                (Some(_), Some(_)) => StatusKind::Modified,
                (Some(_), None) => StatusKind::Deleted,
                (None, None) => return Err(Error::Invalid(format!("invalid staged deletion: {path}"))),
            };
            changes.push(StatusEntry { path, kind });
        }
        Ok(changes)
    }

    /// Capture the exact bytes currently on disk for one path; later edits do
    /// not alter the staged snapshot. Missing tracked paths become deletions.
    pub fn add(&self, path: &str) -> Result<()> {
        validate_path(path)?;
        let _guard = self.lock_worktree()?;
        self.recover_locked()?;
        let before = self.state_at(self.head()?)?;
        let blob_id = match self.read_staging_path(path)? {
            Some(bytes) => Some(self.store.put(BLOB, &bytes)?),
            None if before.contains_key(path) => None,
            None => return Err(Error::Invalid(format!("file is neither tracked nor present: {path}"))),
        };
        let prior = before.get(path).copied();
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO meta(key,value) VALUES ('staging_mode','enabled')
             ON CONFLICT(key) DO UPDATE SET value='enabled'", []
        )?;
        if prior == blob_id {
            tx.execute("DELETE FROM stage_index WHERE path=?1", [path])?;
        } else {
            tx.execute(
                "INSERT INTO stage_index(path,blob_id) VALUES(?1,?2)
                 ON CONFLICT(path) DO UPDATE SET blob_id=excluded.blob_id",
                params![path, blob_id.map(|id| id.to_string())]
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn unstage(&self, path: &str) -> Result<()> {
        validate_path(path)?;
        let _guard = self.lock_worktree()?;
        self.recover_locked()?;
        self.db.execute("DELETE FROM stage_index WHERE path=?1", [path])?;
        Ok(())
    }

    /// A rebuildable derived index. First use on a legacy/stale branch
    /// reconstructs its first-parent snapshot; subsequent staged commits
    /// update only changed paths in the same transaction as the ref CAS.
    fn ensure_branch_path_index(
        &self, branch: &str, tip: Option<ObjectId>
    ) -> Result<()> {
        let expected = tip.map(|id| id.to_string()).unwrap_or_default();
        let current: Option<String> = self.db.query_row(
            "SELECT oid FROM branch_path_index_tip WHERE branch=?1",
            [branch], |r| r.get(0),
        ).optional()?;
        if current.as_deref() == Some(expected.as_str()) {
            return Ok(());
        }
        let full_state = self.state_at(tip)?;
        let tx = self.db.unchecked_transaction()?;
        tx.execute("DELETE FROM branch_path_index WHERE branch=?1", [branch])?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO branch_path_index(branch,path,oid) VALUES(?1,?2,?3)"
            )?;
            for (path, oid) in full_state {
                insert.execute(params![branch, path, oid.to_string()])?;
            }
        }
        tx.execute(
            "INSERT INTO branch_path_index_tip(branch,oid) VALUES(?1,?2)
             ON CONFLICT(branch) DO UPDATE SET oid=excluded.oid",
            params![branch, expected],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn indexed_blob(&self, branch: &str, path: &str) -> Result<Option<ObjectId>> {
        let value: Option<String> = self.db.query_row(
            "SELECT oid FROM branch_path_index WHERE branch=?1 AND path=?2",
            params![branch, path], |r| r.get(0),
        ).optional()?;
        value.map(|raw| ObjectId::parse(&raw)).transpose()
    }

    fn staged_changes_indexed(&self, branch: &str) -> Result<Vec<Change>> {
        let mut changes = Vec::new();
        for (path, blob) in self.staged_blobs()? {
            let previous = self.indexed_blob(branch, &path)?;
            if previous == blob { continue; }
            let kind = if blob.is_none() { ChangeKind::Delete }
                else if previous.is_none() { ChangeKind::Add }
                else { ChangeKind::Modify };
            if let Some(id) = blob {
                let raw = self.store.get(id)?;
                if decode_object(&raw)?.0 != BLOB {
                    return Err(Error::Invalid(format!("staged object is not a blob: {path}")));
                }
            }
            changes.push(Change { path, kind, blob });
        }
        Ok(changes)
    }

    /// Indexed collision check only touches ancestors and descendants of
    /// staged changes; it does not enumerate the entire tracked path set.
    fn validate_staged_tree_indexed(
        &self, branch: &str, changes: &[Change]
    ) -> Result<()> {
        let staged: BTreeMap<&str, &Change> = changes.iter()
            .map(|change| (change.path.as_str(), change)).collect();
        for change in changes {
            if change.kind == ChangeKind::Delete { continue; }
            for (idx, ch) in change.path.char_indices() {
                if ch != '/' { continue; }
                let ancestor = &change.path[..idx];
                if let Some(other) = staged.get(ancestor) {
                    if other.kind != ChangeKind::Delete {
                        return Err(Error::Conflict(format!(
                            "staged file/directory collision: {}", change.path
                        )));
                    }
                } else if self.indexed_blob(branch, ancestor)?.is_some() {
                    return Err(Error::Conflict(format!(
                        "tracked file/directory collision: {}", change.path
                    )));
                }
            }
            let lower = format!("{}/", change.path);
            // ASCII '/' sorts immediately below ASCII '0' in SQLite BINARY
            // collation, giving an indexed range for descendants only.
            let upper = format!("{}0", change.path);
            let mut stmt = self.db.prepare(
                "SELECT path FROM branch_path_index
                 WHERE branch=?1 AND path>=?2 AND path<?3"
            )?;
            let paths = stmt.query_map(params![branch, lower, upper], |r| r.get::<_, String>(0))?;
            for entry in paths {
                let child = entry?;
                if !staged.get(child.as_str()).is_some_and(|other| other.kind == ChangeKind::Delete) {
                    return Err(Error::Conflict(format!(
                        "staged file/directory collision: {}", change.path
                    )));
                }
            }
        }
        Ok(())
    }

    fn staged_changes(&self, old: &BTreeMap<String, ObjectId>) -> Result<Vec<Change>> {
        let mut changes = Vec::new();
        for (path, blob) in self.staged_blobs()? {
            let previous = old.get(&path).copied();
            if previous == blob { continue; }
            let kind = if blob.is_none() { ChangeKind::Delete }
                else if previous.is_none() { ChangeKind::Add }
                else { ChangeKind::Modify };
            if let Some(id) = blob {
                let raw = self.store.get(id)?;
                if decode_object(&raw)?.0 != BLOB {
                    return Err(Error::Invalid(format!("staged object is not a blob: {path}")));
                }
            }
            changes.push(Change { path, kind, blob });
        }
        Ok(changes)
    }

    /// Reject impossible file/directory combinations in the resulting tree.
    /// A selective commit must not leave a tracked file alongside its child.
    fn validate_staged_tree(
        &self, original: &BTreeMap<String, ObjectId>, changes: &[Change]
    ) -> Result<()> {
        let mut paths: BTreeSet<String> = original.keys().cloned().collect();
        for change in changes {
            match change.kind {
                ChangeKind::Delete => { paths.remove(&change.path); }
                ChangeKind::Add | ChangeKind::Modify => { paths.insert(change.path.clone()); }
            }
        }
        for path in &paths {
            for (idx, ch) in path.char_indices() {
                if ch == '/' && paths.contains(&path[..idx]) {
                    return Err(Error::Conflict(format!(
                        "staged snapshot contains a file/directory collision: {path}"
                    )));
                }
            }
        }
        Ok(())
    }

    fn publish_staged_commit(
        &self, branch: &str, previous: Option<ObjectId>, next: ObjectId
    ) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let moved = tx.execute(
            "UPDATE refs SET oid=?1 WHERE name=?2 AND oid IS ?3",
            params![next.to_string(), branch, previous.map(|id| id.to_string())]
        )?;
        if moved != 1 {
            return Err(Error::Conflict(format!("branch {branch} moved; staged files retained")));
        }
        tx.execute("DELETE FROM stage_index", [])?;
        tx.commit()?;
        Ok(())
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


    /// Compare the current worktree with HEAD without modifying the repository.
    /// The initial implementation scans full files and is limited by the v1 blob cap.
    pub fn diff_worktree(&self) -> Result<Vec<FileDiff>> {
        let previous = self.state_at(self.head()?)?;
        let files = self.scan_files()?;
        let mut paths = BTreeSet::new();
        paths.extend(previous.keys().cloned());
        paths.extend(files.keys().cloned());
        let mut result = Vec::new();
        for path in paths {
            let before = previous.get(&path);
            let after = files.get(&path);
            let after_id = after.map(|bytes| encode_object(BLOB, bytes).map(|encoded| object_id(&encoded))).transpose()?;
            if before.copied() == after_id { continue; }
            let old = before.map(|id| self.blob_bytes(*id)).transpose()?;
            result.push(FileDiff { path, before: old, after: after.cloned() });
        }
        Ok(result)
    }

    /// Construct the next commit snapshot without modifying HEAD, the
    /// staging index or the working tree.
    fn staged_state(&self) -> Result<BTreeMap<String, ObjectId>> {
        let mut state = self.state_at(self.head()?)?;
        if self.staging_enabled()? {
            for (path, value) in self.staged_blobs()? {
                if let Some(id) = value {
                    state.insert(path, id);
                } else {
                    state.remove(&path);
                }
            }
        }
        Ok(state)
    }

    /// Show the exact content the next explicitly staged commit would record.
    /// In legacy commit-all mode this is empty because nothing is staged.
    pub fn diff_staged(&self) -> Result<Vec<FileDiff>> {
        if !self.staging_enabled()? { return Ok(Vec::new()); }
        let base = self.state_at(self.head()?)?;
        let staged = self.staged_state()?;
        let mut paths = BTreeSet::new();
        paths.extend(base.keys().cloned());
        paths.extend(staged.keys().cloned());
        let mut result = Vec::new();
        for path in paths {
            let before = base.get(&path);
            let after = staged.get(&path);
            if before == after { continue; }
            result.push(FileDiff {
                path,
                before: before.map(|id| self.blob_bytes(*id)).transpose()?,
                after: after.map(|id| self.blob_bytes(*id)).transpose()?,
            });
        }
        Ok(result)
    }

    /// Compare working files against the staged snapshot, not HEAD. Files
    /// captured by 'add' are thus only shown when edited *after* staging.
    pub fn diff_unstaged(&self) -> Result<Vec<FileDiff>> {
        let staged = self.staged_state()?;
        let files = self.scan_files()?;
        let mut paths = BTreeSet::new();
        paths.extend(staged.keys().cloned());
        paths.extend(files.keys().cloned());
        let mut result = Vec::new();
        for path in paths {
            let before = staged.get(&path);
            let after = files.get(&path);
            let after_id = after.map(|bytes|
                encode_object(BLOB, bytes).map(|encoded| object_id(&encoded))
            ).transpose()?;
            if before.copied() == after_id { continue; }
            result.push(FileDiff {
                path,
                before: before.map(|id| self.blob_bytes(*id)).transpose()?,
                after: after.cloned(),
            });
        }
        Ok(result)
    }

    /// Accurate post-stage working tree status. This is informational;
    /// safety gates still use the full HEAD-to-worktree status.
    pub fn unstaged_status(&self) -> Result<Vec<StatusEntry>> {
        Ok(self.diff_unstaged()?.into_iter().map(|change| {
            let kind = match (change.before.is_some(), change.after.is_some()) {
                (false, true) => StatusKind::Added,
                (true, false) => StatusKind::Deleted,
                _ => StatusKind::Modified,
            };
            StatusEntry { path: change.path, kind }
        }).collect())
    }

    /// Compare two branch tips without materializing either worktree.
    pub fn diff_branches(&self, from: &str, to: &str) -> Result<Vec<FileDiff>> {
        let left = self.state_at(self.branch_tip(from)?)?;
        let right = self.state_at(self.branch_tip(to)?)?;
        let mut paths = BTreeSet::new();
        paths.extend(left.keys().cloned());
        paths.extend(right.keys().cloned());
        let mut result = Vec::new();
        for path in paths {
            let before = left.get(&path);
            let after = right.get(&path);
            if before == after { continue; }
            result.push(FileDiff {
                path,
                before: before.map(|id| self.blob_bytes(*id)).transpose()?,
                after: after.map(|id| self.blob_bytes(*id)).transpose()?,
            });
        }
        Ok(result)
    }

    /// Read native snapshot content by branch and relative path, with no checkout.
    /// Returns None when a file is absent from that branch.
    pub fn read_branch_file(&self, branch: &str, path: &str) -> Result<Option<Vec<u8>>> {
        validate_path(path)?;
        let state = self.state_at(self.branch_tip(branch)?)?;
        state.get(path).copied().map(|id| self.blob_bytes(id)).transpose()
    }

    fn blob_bytes(&self, id: ObjectId) -> Result<Vec<u8>> {
        let raw = self.store.get(id)?;
        let (kind, payload) = decode_object(&raw)?;
        if kind != BLOB { return Err(Error::Invalid(format!("expected blob object: {id}"))); }
        Ok(payload.to_vec())
    }

    pub fn commit(&self, author: &str, message: &str) -> Result<ObjectId> {
        let _worktree_guard = self.lock_worktree()?;
        self.recover_locked()?;
        if author.trim().is_empty() || message.trim().is_empty() { return Err(Error::Invalid("author and commit message are required".into())); }
        let branch = self.current_branch()?;
        let prev = self.branch_tip(&branch)?;
        let old = self.state_at(prev)?;
        let staged = self.staging_enabled()?;
        let changes = if staged {
            // Staged bytes already have immutable object IDs. Reading the
            // entire worktree again would make a small commit O(all files).
            self.staged_changes(&old)?
        } else {
            let files = self.scan_files()?;
            self.make_delta(&old, &files, true)?
        };
        if changes.is_empty() {
            return Err(Error::Invalid(if staged { "nothing staged to commit" } else { "nothing to commit" }.into()));
        }
        if staged { self.validate_staged_tree(&old, &changes)?; }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| Error::Invalid("system time before epoch".into()))?;
        let c = Changeset { parents: prev.into_iter().collect(), author: author.into(), message: message.into(),
            timestamp_seconds: now.as_secs() as i64, changes };
        let raw = c.encode()?;
        let (_, payload) = decode_object(&raw)?;
        let id = self.store.put(CHANGESET, payload)?;
        if staged {
            self.publish_staged_commit(&branch, prev, id)?;
        } else {
            self.save_ref_cas(&branch, prev, id)?;
        }
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
        // Reject file/directory collisions and unexpected files before touching the worktree.
        for path in target.keys() {
            for (idx, ch) in path.char_indices() {
                if ch == '/' && target.contains_key(&path[..idx]) {
                    return Err(Error::Conflict(format!("file/directory collision: {path}")));
                }
            }
        }
        let actual = self.scan_files()?;
        let mut known = BTreeSet::new();
        known.extend(previous.keys().cloned());
        known.extend(target.keys().cloned());
        for path in actual.keys() {
            if !known.contains(path) {
                return Err(Error::Conflict(format!("untracked file blocks recovery: {path}")));
            }
        }
        for path in &known {
            let before = previous.get(path).copied();
            let after = target.get(path).copied();
            if let Some(bytes) = actual.get(path) {
                let encoded = encode_object(BLOB, bytes)?;
                let current = object_id(&encoded);
                if Some(current) != before && Some(current) != after {
                    return Err(Error::Conflict(format!("modified file blocks recovery: {path}")));
                }
            } else if before.is_some() && after.is_some() {
                return Err(Error::Conflict(format!("missing tracked file blocks recovery: {path}")));
            }
        }
        // Verify every required blob before removing anything.
        for id in target.values() { self.blob_bytes(*id)?; }

        for path in previous.keys() {
            if target.contains_key(path) { continue; }
            let full = self.root.join(path);
            match fs::symlink_metadata(&full) {
                Ok(meta) if meta.is_file() => fs::remove_file(&full)?,
                Ok(meta) if meta.is_dir() && target.keys().any(|p| p.starts_with(&format!("{path}/"))) => {},
                Ok(_) => return Err(Error::Conflict(format!("unexpected path type during recovery: {path}"))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                Err(e) => return Err(e.into()),
            }
        }
        let mut dirs: Vec<PathBuf> = previous.keys()
            .filter_map(|p| self.root.join(p).parent().map(Path::to_path_buf)).collect();
        dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        dirs.dedup();
        for dir in dirs {
            if dir == self.root { continue; }
            match fs::remove_dir(&dir) {
                Ok(()) => {},
                Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty) => {},
                Err(e) => return Err(e.into()),
            }
        }

        // Stage each file under .loom, fsync it and atomically rename into place.
        // A crash can leave old/new mixtures but never a partially written tracked file.
        let staging = self.root.join(".loom").join("staging");
        fs::create_dir_all(&staging)?;
        for (path, oid) in target {
            let already_correct = actual.get(path).is_some_and(|bytes| {
                encode_object(BLOB, bytes).is_ok_and(|encoded| object_id(&encoded) == *oid)
            });
            if already_correct { continue; }
            let bytes = self.blob_bytes(*oid)?;
            let full = self.root.join(path);
            if let Some(parent) = full.parent() { fs::create_dir_all(parent)?; }
            let mut tempfile = None;
            for _ in 0..16 {
                let nonce = STAGE_NONCE.fetch_add(1, Ordering::Relaxed);
                let candidate = staging.join(format!("{}-{nonce}", std::process::id()));
                match fs::OpenOptions::new().create_new(true).write(true).open(&candidate) {
                    Ok(file) => { tempfile = Some((candidate, file)); break; }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {},
                    Err(e) => return Err(e.into()),
                }
            }
            let (temp, mut file) = tempfile.ok_or_else(|| Error::Conflict("unable to allocate a staging file".into()))?;
            use std::io::Write;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, &full)?;
            #[cfg(unix)]
            if let Some(parent) = full.parent() { fs::File::open(parent)?.sync_all()?; }
        }
        Ok(())
    }
    pub fn switch_branch(&self, name: &str) -> Result<()> {
        let _worktree_guard = self.lock_worktree()?;
        self.recover_locked()?;
        if !self.staged_blobs()?.is_empty() {
            return Err(Error::Conflict("unstage or commit staged changes before switching branches".into()));
        }
        if !self.status()?.is_empty() {
            return Err(Error::Conflict("working tree has uncommitted or untracked files".into()));
        }
        let old_branch = self.current_branch()?;
        let before = self.head()?;
        let after = self.branch_tip(name)?;
        if old_branch == name { return Ok(()); }
        for id in self.state_at(after)?.values() { self.blob_bytes(*id)?; }
        self.begin_transition(&PendingTransition {
            operation: "switch".into(),
            from_branch: old_branch,
            to_branch: name.into(),
            before,
            after,
        })?;
        self.recover_locked()
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

    /// Conservative merge: independent lines merge, ambiguous changes reject safely.
    /// Ambiguous edits are rejected without creating a merge commit or changing the worktree.
    /// This is not yet a complete rename-aware or structured conflict engine.
    pub fn merge_branch(&self, name: &str, author: &str, message: &str) -> Result<ObjectId> {
        let _worktree_guard = self.lock_worktree()?;
        self.recover_locked()?;
        if !self.staged_blobs()?.is_empty() {
            return Err(Error::Conflict("unstage or commit staged changes before merging".into()));
        }
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
            let chosen = if local == remote { local.copied() }
                else if local == original { remote.copied() }
                else if remote == original { local.copied() }
                else {
                    match (original, local, remote) {
                        (Some(base_id), Some(our_id), Some(their_id)) => {
                            let base = self.blob_bytes(*base_id)?;
                            let our_bytes = self.blob_bytes(*our_id)?;
                            let their_bytes = self.blob_bytes(*their_id)?;
                            if let Some(bytes) = crate::text_merge::merge_text(&base, &our_bytes, &their_bytes) {
                                Some(self.store.put(BLOB, &bytes)?)
                            } else {
                                conflicts.push(path.clone());
                                continue;
                            }
                        }
                        _ => { conflicts.push(path.clone()); continue }
                    }
                };
            if let Some(id) = chosen { merged.insert(path, id); }
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
        self.begin_transition(&PendingTransition {
            operation: "merge".into(),
            from_branch: branch.clone(),
            to_branch: branch,
            before: Some(ours),
            after: Some(id),
        })?;
        self.recover_locked()?;
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

    #[test]
    fn recovers_interrupted_switch_from_mixed_file_state() {
        let (dir, repo) = repo();
        fs::write(dir.path().join("a.txt"), b"base a").unwrap();
        fs::write(dir.path().join("b.txt"), b"base b").unwrap();
        repo.commit("tester", "baseline").unwrap();
        repo.create_branch("feature").unwrap();
        repo.switch_branch("feature").unwrap();
        fs::write(dir.path().join("a.txt"), b"feature a").unwrap();
        fs::write(dir.path().join("b.txt"), b"feature b").unwrap();
        repo.commit("tester", "updated").unwrap();
        repo.switch_branch("main").unwrap();

        let current = repo.head().unwrap();
        let next = repo.branch_tip("feature").unwrap();
        repo.begin_transition(&PendingTransition {
            operation: "switch".into(), from_branch: "main".into(),
            to_branch: "feature".into(), before: current, after: next,
        }).unwrap();
        // Imitate a terminated process: only the first file made it to disk.
        fs::write(dir.path().join("a.txt"), b"feature a").unwrap();
        drop(repo);

        let reopened = Repository::discover(dir.path()).unwrap();
        assert_eq!(reopened.current_branch().unwrap(), "feature");
        assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"feature a");
        assert_eq!(fs::read(dir.path().join("b.txt")).unwrap(), b"feature b");
        assert!(reopened.pending_transition().unwrap().is_none());
        assert!(reopened.status().unwrap().is_empty());
    }

    #[test]
    fn recovery_never_discards_untracked_user_files() {
        let (dir, repo) = repo();
        fs::write(dir.path().join("a"), b"v1").unwrap();
        repo.commit("tester", "baseline").unwrap();
        repo.create_branch("feature").unwrap();
        repo.begin_transition(&PendingTransition {
            operation: "switch".into(), from_branch: "main".into(),
            to_branch: "feature".into(), before: repo.head().unwrap(),
            after: repo.branch_tip("feature").unwrap(),
        }).unwrap();
        drop(repo);
        fs::write(dir.path().join("user.txt"), b"untouched").unwrap();
        assert!(Repository::discover(dir.path()).is_err());
        assert_eq!(fs::read(dir.path().join("user.txt")).unwrap(), b"untouched");
        fs::remove_file(dir.path().join("user.txt")).unwrap();
        let reopened = Repository::discover(dir.path()).unwrap();
        assert_eq!(reopened.current_branch().unwrap(), "feature");
    }

    #[test]
    fn switch_handles_file_directory_transitions_in_both_directions() {
        let (dir, repo) = repo();
        fs::write(dir.path().join("node"), b"one").unwrap();
        repo.commit("tester", "node is file").unwrap();
        repo.create_branch("directory").unwrap();
        repo.switch_branch("directory").unwrap();
        fs::remove_file(dir.path().join("node")).unwrap();
        fs::create_dir(dir.path().join("node")).unwrap();
        fs::write(dir.path().join("node").join("leaf"), b"two").unwrap();
        repo.commit("tester", "node is directory").unwrap();
        repo.switch_branch("main").unwrap();
        assert_eq!(fs::read(dir.path().join("node")).unwrap(), b"one");
        repo.switch_branch("directory").unwrap();
        assert_eq!(fs::read(dir.path().join("node").join("leaf")).unwrap(), b"two");
        assert!(repo.status().unwrap().is_empty());
    }

    #[test]
    fn recovery_refuses_user_modifications_without_moving_head() {
        let (dir, repo) = repo();
        fs::write(dir.path().join("file"), b"main").unwrap();
        repo.commit("tester", "base").unwrap();
        repo.create_branch("feature").unwrap();
        repo.switch_branch("feature").unwrap();
        fs::write(dir.path().join("file"), b"feature").unwrap();
        repo.commit("tester", "feature").unwrap();
        repo.switch_branch("main").unwrap();
        let before = repo.head().unwrap();
        repo.begin_transition(&PendingTransition {
            operation: "switch".into(), from_branch: "main".into(),
            to_branch: "feature".into(), before, after: repo.branch_tip("feature").unwrap(),
        }).unwrap();
        drop(repo);
        fs::write(dir.path().join("file"), b"user edit").unwrap();
        assert!(Repository::discover(dir.path()).is_err());
        assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"user edit");
        // The pending intent survives and can be resumed only after manual repair.
        fs::write(dir.path().join("file"), b"main").unwrap();
        let reopened = Repository::discover(dir.path()).unwrap();
        assert_eq!(reopened.current_branch().unwrap(), "feature");
        assert!(reopened.status().unwrap().is_empty());
    }

    #[test]
    fn merge_journal_publishes_ref_only_after_materialization() {
        let (dir, repo) = repo();
        fs::write(dir.path().join("a"), b"base").unwrap();
        repo.commit("tester", "base").unwrap();
        let first = repo.head().unwrap().unwrap();
        let blob = repo.store.put(BLOB, b"merged").unwrap();
        let merged = Changeset {
            parents: vec![first, first], author: "tester".into(), message: "merge".into(),
            timestamp_seconds: 1,
            changes: vec![Change { kind: ChangeKind::Modify, path: "a".into(), blob: Some(blob) }],
        };
        let bytes = merged.encode().unwrap();
        let (_, payload) = decode_object(&bytes).unwrap();
        let merged_id = repo.store.put(CHANGESET, payload).unwrap();
        repo.begin_transition(&PendingTransition {
            operation: "merge".into(), from_branch: "main".into(), to_branch: "main".into(),
            before: Some(first), after: Some(merged_id),
        }).unwrap();
        assert_eq!(repo.head().unwrap(), Some(first));
        drop(repo);
        let reopened = Repository::discover(dir.path()).unwrap();
        assert_eq!(reopened.head().unwrap(), Some(merged_id));
        assert_eq!(fs::read(dir.path().join("a")).unwrap(), b"merged");
        assert!(reopened.pending_transition().unwrap().is_none());
        assert!(reopened.status().unwrap().is_empty());
    }

    #[test] fn corrupt_object_detected() {
        let (_dir, r) = repo();
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
