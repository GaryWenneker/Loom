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
             CREATE TABLE IF NOT EXISTS revision_file_cache (
                 revision_oid TEXT NOT NULL,
                 path TEXT NOT NULL,
                 blob_oid TEXT,
                 PRIMARY KEY (revision_oid, path)
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

    /// Report prepared file changes without replaying the entire first-parent
    /// chain when the derived branch path index covers the current ref.
    /// A single SQLite snapshot pins the ref and its index revision marker.
    pub fn staged_status(&self) -> Result<Vec<StatusEntry>> {
        let staged = self.staged_blobs()?;
        if staged.is_empty() { return Ok(Vec::new()); }
        let branch = self.current_branch()?;
        let tx = self.db.unchecked_transaction()?;
        let (ref_oid, index_oid): (Option<String>, Option<String>) = tx.query_row(
            "SELECT r.oid,t.oid FROM refs AS r
             LEFT JOIN branch_path_index_tip AS t ON t.branch=r.name
             WHERE r.name=?1",
            [&branch], |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let current = index_oid.as_deref() == Some(ref_oid.as_deref().unwrap_or(""));
        if current {
            let mut query = tx.prepare(
                "SELECT 1 FROM branch_path_index WHERE branch=?1 AND path=?2"
            )?;
            let mut changes = Vec::with_capacity(staged.len());
            for (path, blob) in staged {
                let tracked: Option<i64> = query.query_row(
                    params![&branch,&path], |row| row.get(0),
                ).optional()?;
                let kind = match (tracked.is_some(), blob) {
                    (false, Some(_)) => StatusKind::Added,
                    (true, Some(_)) => StatusKind::Modified,
                    (true, None) => StatusKind::Deleted,
                    (false, None) => return Err(Error::Invalid(format!(
                        "invalid staged deletion: {path}"
                    ))),
                };
                changes.push(StatusEntry { path, kind });
            }
            drop(query);
            tx.commit()?;
            return Ok(changes);
        }
        // A legacy/merge-written ref may not have an index, so immutable
        // changeset history remains the canonical fallback.
        drop(tx);
        let ref_id = ref_oid.map(|s| ObjectId::parse(&s)).transpose()?;
        let old = self.state_at(ref_id)?;
        let mut changes = Vec::with_capacity(staged.len());
        for (path, blob) in staged {
            let kind = match (old.get(&path), blob) {
                (None, Some(_)) => StatusKind::Added,
                (Some(_), Some(_)) => StatusKind::Modified,
                (Some(_), None) => StatusKind::Deleted,
                (None, None) => return Err(Error::Invalid(format!(
                    "invalid staged deletion: {path}"
                ))),
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
        let branch = self.current_branch()?;
        let tip = self.branch_tip(&branch)?;
        // Lazily rebuild a missing or stale derived index once. Subsequent
        // adds only need one indexed lookup, irrespective of history depth.
        self.ensure_branch_path_index(&branch, tip)?;
        let prior = self.indexed_blob(&branch, path)?;
        let blob_id = match self.read_staging_path(path)? {
            Some(bytes) => Some(self.store.put(BLOB, &bytes)?),
            None if prior.is_some() => None,
            None => return Err(Error::Invalid(format!("file is neither tracked nor present: {path}"))),
        };
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

    fn publish_staged_commit(
        &self, branch: &str, previous: Option<ObjectId>, next: ObjectId,
        changes: &[Change]
    ) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let moved = tx.execute(
            "UPDATE refs SET oid=?1 WHERE name=?2 AND oid IS ?3",
            params![next.to_string(), branch, previous.map(|id| id.to_string())]
        )?;
        if moved != 1 {
            return Err(Error::Conflict(format!("branch {branch} moved; staged files retained")));
        }
        // Keep the derived path cache and its revision marker consistent with
        // refs in the SAME SQLite transaction. An interrupted commit cannot
        // publish a new ref alongside old cached file state.
        for change in changes {
            match change.kind {
                ChangeKind::Delete => {
                    tx.execute(
                        "DELETE FROM branch_path_index WHERE branch=?1 AND path=?2",
                        params![branch, change.path],
                    )?;
                }
                ChangeKind::Add | ChangeKind::Modify => {
                    let id = change.blob.ok_or_else(||
                        Error::Invalid("staged update without blob".into())
                    )?;
                    tx.execute(
                        "INSERT INTO branch_path_index(branch,path,oid)
                         VALUES(?1,?2,?3)
                         ON CONFLICT(branch,path) DO UPDATE SET oid=excluded.oid",
                        params![branch, change.path, id.to_string()],
                    )?;
                }
            }
        }
        tx.execute(
            "UPDATE branch_path_index_tip SET oid=?1 WHERE branch=?2",
            params![next.to_string(), branch],
        )?;
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
    /// Enumerate tracked native paths without materializing a worktree.
    /// A prefix matches the exact file name and descendants of that directory,
    /// but never similarly named siblings (e.g. 'src2' for prefix 'src').
    ///
    /// One SQLite read transaction pins the same snapshot for the ref marker
    /// and indexed path reads. Prefix lookups use the (branch,path) index for
    /// one exact lookup plus one bounded range, not a full branch path scan.
    pub fn list_branch_files(
        &self, branch: &str, prefix: Option<&str>
    ) -> Result<Vec<String>> {
        if let Some(path) = prefix { validate_path(path)?; }
        let tx = self.db.unchecked_transaction()?;
        let current: Option<i64> = tx.query_row(
            "SELECT 1 FROM refs AS r
             JOIN branch_path_index_tip AS t
               ON t.branch=r.name AND t.oid=COALESCE(r.oid,'')
             WHERE r.name=?1",
            [branch],
            |row| row.get(0),
        ).optional()?;
        if current.is_some() {
            let mut paths = Vec::new();
            if let Some(p) = prefix {
                let lower = format!("{p}/");
                let upper = format!("{p}0");
                let mut stmt = tx.prepare(
                    "SELECT path FROM branch_path_index
                       WHERE branch=?1 AND path=?2
                     UNION ALL
                     SELECT path FROM branch_path_index
                       WHERE branch=?1 AND path>=?3 AND path<?4
                     ORDER BY path"
                )?;
                let rows = stmt.query_map(
                    params![branch, p, lower, upper],
                    |row| row.get::<_, String>(0),
                )?;
                for row in rows {
                    let path = row?;
                    validate_path(&path)?;
                    paths.push(path);
                }
            } else {
                let mut stmt = tx.prepare(
                    "SELECT path FROM branch_path_index
                     WHERE branch=?1 ORDER BY path"
                )?;
                let rows = stmt.query_map([branch], |row| row.get::<_, String>(0))?;
                for row in rows {
                    let path = row?;
                    validate_path(&path)?;
                    paths.push(path);
                }
            }
            tx.commit()?;
            return Ok(paths);
        }
        drop(tx);
        // A legacy branch, new branch, or stale/evicted cache is served from
        // immutable first-parent history without treating derived data as
        // authoritative. New staged commits rebuild the index when needed.
        let history = self.state_at(self.branch_tip(branch)?)?;
        let directory_prefix = prefix.map(|p| format!("{p}/"));
        Ok(history.into_keys().filter(|path| {
            prefix.is_none()
                || prefix.is_some_and(|p| path.as_str() == p)
                || directory_prefix.as_ref().is_some_and(|p| path.starts_with(p.as_str()))
        }).collect())
    }

    /// Resolve one file at a historical changeset without constructing the
    /// complete tree. v1 deltas use the first parent as their state base.
    /// The cache stores query results only; immutable changesets are the
    /// authoritative source and allow rebuilding the cache at any time.
    fn resolve_revision_path(
        &self, revision: ObjectId, path: &str
    ) -> Result<Option<ObjectId>> {
        let mut cursor = Some(revision);
        let mut visited = HashSet::new();
        while let Some(id) = cursor {
            if visited.len() >= 1_000_000 {
                return Err(Error::Invalid("history depth limit reached".into()));
            }
            if !visited.insert(id) {
                return Err(Error::Invalid("cycle in first-parent history".into()));
            }
            let commit = self.get_changeset(id)?;
            // Canonical changesets always contain sorted, unique path entries.
            if let Ok(pos) = commit.changes.binary_search_by(|c| c.path.as_str().cmp(path)) {
                return Ok(commit.changes[pos].blob);
            }
            cursor = commit.parents.first().copied();
        }
        Ok(None)
    }

    /// Return a file's bytes at any native changeset ID, without checking out
    /// the revision. Cache hits require one indexed SQLite lookup and the
    /// requested immutable blob read; a cache miss walks only change metadata
    /// along the first-parent chain rather than materializing all paths.
    pub fn read_revision_file(
        &self, revision: ObjectId, path: &str
    ) -> Result<Option<Vec<u8>>> {
        validate_path(path)?;
        // A cache hit must not turn a missing/non-changeset revision into a
        // valid lookup. The requested revision itself is always checked.
        self.get_changeset(revision)?;
        let key = revision.to_string();
        let cached: Option<Option<String>> = self.db.query_row(
            "SELECT blob_oid FROM revision_file_cache
             WHERE revision_oid=?1 AND path=?2",
            params![&key, path], |row| row.get(0),
        ).optional()?;
        let blob = if let Some(value) = cached {
            value.map(|raw| ObjectId::parse(&raw)).transpose()?
        } else {
            let resolved = self.resolve_revision_path(revision, path)?;
            self.db.execute(
                "INSERT OR IGNORE INTO revision_file_cache(revision_oid,path,blob_oid)
                 VALUES(?1,?2,?3)",
                params![&key, path, resolved.map(|id| id.to_string())],
            )?;
            resolved
        };
        blob.map(|id| self.blob_bytes(id)).transpose()
    }

    /// Read a file from a branch tip using the derived path index when it
    /// represents the same commit as the live ref. The joined lookup uses
    /// one SQLite read snapshot, so a concurrent ref/index transaction cannot
    /// mix a new branch tip with old cached paths.
    ///
    /// An absent or stale index is never trusted. The native immutable
    /// history remains authoritative and can reconstruct the requested file.
    pub fn read_branch_file(&self, branch: &str, path: &str) -> Result<Option<Vec<u8>>> {
        validate_path(path)?;
        let cached: Option<Option<String>> = self.db.query_row(
            "SELECT i.oid FROM refs AS r
             JOIN branch_path_index_tip AS t
               ON t.branch=r.name AND t.oid=COALESCE(r.oid,'')
             LEFT JOIN branch_path_index AS i
               ON i.branch=r.name AND i.path=?2
             WHERE r.name=?1",
            params![branch, path],
            |row| row.get(0),
        ).optional()?;
        match cached {
            Some(Some(id)) => return self.blob_bytes(ObjectId::parse(&id)?).map(Some),
            Some(None) => return Ok(None),
            None => {},
        }
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
        let staged = self.staging_enabled()?;
        let changes = if staged {
            // On first use or after a merge/legacy commit, rebuild from
            // immutable history. Subsequent staged commits use indexed
            // lookups and touch only paths named by the staged snapshot.
            self.ensure_branch_path_index(&branch, prev)?;
            let changes = self.staged_changes_indexed(&branch)?;
            self.validate_staged_tree_indexed(&branch, &changes)?;
            changes
        } else {
            let old = self.state_at(prev)?;
            let files = self.scan_files()?;
            self.make_delta(&old, &files, true)?
        };
        if changes.is_empty() {
            return Err(Error::Invalid(if staged { "nothing staged to commit" } else { "nothing to commit" }.into()));
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| Error::Invalid("system time before epoch".into()))?;
        let c = Changeset { parents: prev.into_iter().collect(), author: author.into(), message: message.into(),
            timestamp_seconds: now.as_secs() as i64, changes };
        let raw = c.encode()?;
        let (_, payload) = decode_object(&raw)?;
        let id = self.store.put(CHANGESET, payload)?;
        if staged {
            self.publish_staged_commit(&branch, prev, id, &c.changes)?;
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
        // Staged blobs are independent durability roots: a prepared commit
        // must never silently reference missing, corrupt or wrong-type objects.
        // A deletion tombstone is valid only if the selected path is tracked
        // at the current branch head.
        let staged = self.staged_blobs()?;
        if !staged.is_empty() {
            if !self.staging_enabled()? {
                return Err(Error::Invalid(
                    "staging index has entries but staging mode is disabled".into()
                ));
            }
            let tracked = self.state_at(self.head()?)?;
            for (path, blob) in staged {
                match blob {
                    Some(id) => { self.blob_bytes(id)?; }
                    None if tracked.contains_key(&path) => {},
                    None => return Err(Error::Invalid(format!(
                        "staged deletion does not reference a tracked path: {path}"
                    ))),
                }
            }
        }
        // A pending workspace transition is another integrity root. Even
        // when it has not published the destination ref, its immutable
        // changeset must remain available for safe replay.
        if let Some(pending) = self.pending_transition()? {
            for id in [pending.before, pending.after].into_iter().flatten() {
                self.get_changeset(id)?;
            }
        }
        for (branch, tip) in self.branches()? {
            if let Some(id) = tip { self.get_changeset(id)?; }
            let expected = self.state_at(tip)?;
            // Derived index is allowed to be absent or stale after merge,
            // branch creation or legacy commit, but if its revision marker
            // claims to be current its contents must match immutable history.
            let indexed_tip: Option<String> = self.db.query_row(
                "SELECT oid FROM branch_path_index_tip WHERE branch=?1",
                [&branch], |row| row.get(0),
            ).optional()?;
            let head = tip.map(|id| id.to_string()).unwrap_or_default();
            if indexed_tip.as_deref() == Some(head.as_str()) {
                let mut stmt = self.db.prepare(
                    "SELECT path,oid FROM branch_path_index WHERE branch=?1 ORDER BY path"
                )?;
                let entries = stmt.query_map([&branch], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                let mut indexed = BTreeMap::new();
                for row in entries {
                    let (path, id) = row?;
                    validate_path(&path)?;
                    indexed.insert(path, ObjectId::parse(&id)?);
                }
                if indexed != expected {
                    return Err(Error::Invalid(format!(
                        "derived branch path index disagrees with history: {branch}"
                    )));
                }
            }
        }
        // Cached historical answers are derived data, not new repository
        // authority. fsck detects stale or tampered path results by replaying
        // only the requested path along the immutable first-parent chain.
        let mut statement = self.db.prepare(
            "SELECT revision_oid,path,blob_oid FROM revision_file_cache"
        )?;
        let entries = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?))
        })?;
        for entry in entries {
            let (raw_revision, path, raw_blob) = entry?;
            validate_path(&path)?;
            let revision = ObjectId::parse(&raw_revision)?;
            let cached = raw_blob.map(|v| ObjectId::parse(&v)).transpose()?;
            if self.resolve_revision_path(revision, &path)? != cached {
                return Err(Error::Invalid(format!(
                    "historical path cache differs from immutable changesets: {path}"
                )));
            }
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


    #[test]
    fn derived_index_is_incremental_and_rebuildable() {
        let (dir, r) = repo();
        for i in 0..40 {
            fs::write(dir.path().join(format!("file-{i:03}.txt")), format!("base {i}")).unwrap();
        }
        let baseline = r.commit("tester", "baseline").unwrap();
        // Legacy repository lazily initializes its derived branch index.
        assert!(r.db.query_row(
            "SELECT oid FROM branch_path_index_tip WHERE branch='main'",
            [], |row| row.get::<_, String>(0)
        ).optional().unwrap().is_none());
        for i in 0..12 {
            fs::write(dir.path().join("file-000.txt"), format!("step {i}")).unwrap();
            r.add("file-000.txt").unwrap();
            r.commit("tester", &format!("step {i}")).unwrap();
        }
        let tip = r.head().unwrap().unwrap();
        assert_ne!(tip, baseline);
        let current: String = r.db.query_row(
            "SELECT oid FROM branch_path_index_tip WHERE branch='main'",
            [], |row| row.get(0)
        ).unwrap();
        assert_eq!(current, tip.to_string());
        let size: i64 = r.db.query_row(
            "SELECT count(*) FROM branch_path_index WHERE branch='main'",
            [], |row| row.get(0)
        ).unwrap();
        assert_eq!(size, 40);
        assert_eq!(r.indexed_blob("main", "file-000.txt").unwrap(),
            r.state_at(Some(tip)).unwrap().get("file-000.txt").copied());

        // Drop only derived data. History remains authoritative and an
        // upcoming staged commit reconstructs the index from immutable data.
        r.db.execute("DELETE FROM branch_path_index_tip", []).unwrap();
        r.db.execute("DELETE FROM branch_path_index", []).unwrap();
        fs::write(dir.path().join("file-001.txt"), b"recovered").unwrap();
        r.add("file-001.txt").unwrap();
        r.commit("tester", "after index eviction").unwrap();
        assert!(r.fsck().is_ok());
        assert_eq!(r.read_branch_file("main", "file-001.txt").unwrap(),
            Some(b"recovered".to_vec()));
    }

    #[test]
    fn derived_indexes_are_branch_isolated() {
        let (dir, r) = repo();
        fs::write(dir.path().join("shared"), b"original").unwrap();
        r.commit("tester", "start").unwrap();
        r.create_branch("feature").unwrap();
        r.switch_branch("feature").unwrap();
        fs::write(dir.path().join("shared"), b"feature").unwrap();
        r.add("shared").unwrap();
        r.commit("tester", "feature version").unwrap();
        assert_eq!(r.read_branch_file("feature", "shared").unwrap(),
            Some(b"feature".to_vec()));
        r.switch_branch("main").unwrap();
        fs::write(dir.path().join("shared"), b"main").unwrap();
        r.add("shared").unwrap();
        r.commit("tester", "main version").unwrap();
        assert_eq!(r.read_branch_file("main", "shared").unwrap(),
            Some(b"main".to_vec()));
        assert_eq!(r.read_branch_file("feature", "shared").unwrap(),
            Some(b"feature".to_vec()));
        assert!(r.fsck().is_ok());
    }


    #[test]
    fn branch_file_reads_follow_indexed_commits_additions_and_deletions() {
        let (dir, r) = repo();
        fs::write(dir.path().join("first.txt"), b"original").unwrap();
        fs::write(dir.path().join("keep.txt"), b"unchanged").unwrap();
        r.commit("tester", "initial").unwrap();
        // Legacy commits have no derived index; history is the fallback.
        assert_eq!(r.read_branch_file("main", "first.txt").unwrap(),
            Some(b"original".to_vec()));
        assert_eq!(r.read_branch_file("main", "missing.txt").unwrap(), None);

        fs::write(dir.path().join("first.txt"), b"indexed").unwrap();
        r.add("first.txt").unwrap();
        r.commit("tester", "create indexed tip").unwrap();
        assert_eq!(r.read_branch_file("main", "first.txt").unwrap(),
            Some(b"indexed".to_vec()));
        assert_eq!(r.read_branch_file("main", "keep.txt").unwrap(),
            Some(b"unchanged".to_vec()));
        assert_eq!(r.read_branch_file("main", "missing.txt").unwrap(), None);

        fs::remove_file(dir.path().join("first.txt")).unwrap();
        r.add("first.txt").unwrap();
        r.commit("tester", "delete indexed path").unwrap();
        assert_eq!(r.read_branch_file("main", "first.txt").unwrap(), None);
        assert_eq!(r.read_branch_file("main", "keep.txt").unwrap(),
            Some(b"unchanged".to_vec()));
        assert!(r.fsck().is_ok());
    }

    #[test]
    fn branch_file_reads_fall_back_when_index_is_stale_or_evicted() {
        let (dir, r) = repo();
        fs::write(dir.path().join("a"), b"one").unwrap();
        r.commit("tester", "base").unwrap();
        fs::write(dir.path().join("a"), b"two").unwrap();
        r.add("a").unwrap();
        r.commit("tester", "indexed change").unwrap();
        let current = r.head().unwrap().unwrap();
        assert_eq!(r.read_branch_file("main", "a").unwrap(),
            Some(b"two".to_vec()));
        // Simulate index eviction: the immutable changesets remain usable.
        r.db.execute("DELETE FROM branch_path_index_tip", []).unwrap();
        assert_eq!(r.read_branch_file("main", "a").unwrap(),
            Some(b"two".to_vec()));
        // A stale marker must not masquerade as the current branch revision.
        r.db.execute(
            "INSERT INTO branch_path_index_tip(branch,oid) VALUES('main',?1)",
            [ObjectId([7; 32]).to_string()],
        ).unwrap();
        assert_eq!(r.read_branch_file("main", "a").unwrap(),
            Some(b"two".to_vec()));
        assert_eq!(r.head().unwrap(), Some(current));
        assert!(r.fsck().is_ok());
    }

    #[test]
    fn branch_file_reads_are_branch_isolated_after_switches() {
        let (dir, r) = repo();
        fs::write(dir.path().join("value"), b"base").unwrap();
        r.commit("tester", "base").unwrap();
        r.create_branch("feature").unwrap();
        r.switch_branch("feature").unwrap();
        fs::write(dir.path().join("value"), b"feature").unwrap();
        r.add("value").unwrap();
        r.commit("tester", "feature commit").unwrap();
        r.switch_branch("main").unwrap();
        fs::write(dir.path().join("value"), b"main").unwrap();
        r.add("value").unwrap();
        r.commit("tester", "main commit").unwrap();
        assert_eq!(r.read_branch_file("feature", "value").unwrap(),
            Some(b"feature".to_vec()));
        assert_eq!(r.read_branch_file("main", "value").unwrap(),
            Some(b"main".to_vec()));
        assert!(r.read_branch_file("does-not-exist", "value").is_err());
        assert!(r.fsck().is_ok());
    }

    #[test]
    fn indexed_native_listing_falls_back_for_evicted_and_stale_caches() {
        let (dir, r) = repo();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/alpha"), b"alpha").unwrap();
        fs::write(dir.path().join("src/beta"), b"beta").unwrap();
        fs::write(dir.path().join("src2"), b"sibling").unwrap();
        r.commit("tester", "base").unwrap();
        assert_eq!(r.list_branch_files("main", Some("src")).unwrap(),
                   vec!["src/alpha", "src/beta"]);
        fs::write(dir.path().join("src/alpha"), b"updated").unwrap();
        r.add("src/alpha").unwrap();
        r.commit("tester", "warm cache").unwrap();
        let expected = vec!["src/alpha".to_owned(), "src/beta".to_owned()];
        assert_eq!(r.list_branch_files("main", Some("src")).unwrap(), expected);
        r.db.execute("DELETE FROM branch_path_index_tip", []).unwrap();
        assert_eq!(r.list_branch_files("main", Some("src")).unwrap(), expected);
        r.db.execute(
            "INSERT INTO branch_path_index_tip(branch,oid) VALUES('main',?1)",
            [ObjectId([9; 32]).to_string()],
        ).unwrap();
        assert_eq!(r.list_branch_files("main", Some("src")).unwrap(), expected);
        assert_eq!(r.list_branch_files("main", Some("src2")).unwrap(),
                   vec!["src2"]);
        assert_eq!(r.list_branch_files("main", Some("missing")).unwrap(),
                   Vec::<String>::new());
        assert!(r.list_branch_files("missing", None).is_err());
        assert!(r.list_branch_files("main", Some("../unsafe")).is_err());
    }


    #[test]
    fn fsck_verifies_staged_content_and_tombstones() {
        let (dir, r) = repo();
        fs::write(dir.path().join("tracked"), b"original").unwrap();
        r.commit("tester", "base").unwrap();
        fs::write(dir.path().join("tracked"), b"prepared").unwrap();
        r.add("tracked").unwrap();
        assert!(r.fsck().is_ok());
        fs::remove_file(dir.path().join("tracked")).unwrap();
        // The staged bytes are immutable and independent of the worktree.
        assert!(r.fsck().is_ok());
        r.unstage("tracked").unwrap();
        r.add("tracked").unwrap();
        assert!(r.fsck().is_ok()); // Valid deletion of a tracked path.
    }

    #[test]
    fn fsck_rejects_missing_and_wrong_type_staged_blobs() {
        let (dir, r) = repo();
        fs::write(dir.path().join("tracked"), b"original").unwrap();
        let changeset = r.commit("tester", "base").unwrap();
        fs::write(dir.path().join("tracked"), b"prepared").unwrap();
        r.add("tracked").unwrap();
        let staged = r.staged_blobs().unwrap();
        let oid = staged.get("tracked").copied().flatten().unwrap();
        fs::remove_file(r.store.path_for_test(oid)).unwrap();
        assert!(r.fsck().is_err(), "missing staged blob must be detected");

        r.db.execute(
            "UPDATE stage_index SET blob_id=?1 WHERE path='tracked'",
            [changeset.to_string()]
        ).unwrap();
        assert!(r.fsck().is_err(), "changeset must not be treated as staged blob");
    }

    #[test]
    fn fsck_rejects_invalid_staging_tombstones() {
        let (dir, r) = repo();
        fs::write(dir.path().join("tracked"), b"contents").unwrap();
        r.commit("tester", "base").unwrap();
        fs::write(dir.path().join("new-file"), b"staged").unwrap();
        r.add("new-file").unwrap();
        r.db.execute(
            "INSERT INTO stage_index(path,blob_id) VALUES('ghost-file',NULL)",
            []
        ).unwrap();
        assert!(r.fsck().is_err());
    }

    #[test]
    fn fsck_validates_objects_retained_by_pending_recovery() {
        let (_dir, r) = repo();
        // The journal is a live root even before it publishes a new ref.
        r.begin_transition(&PendingTransition {
            operation: "merge".into(),
            from_branch: "main".into(),
            to_branch: "main".into(),
            before: None,
            after: Some(ObjectId([9; 32])),
        }).unwrap();
        assert!(r.fsck().is_err());
    }


    #[test]
    fn indexed_add_handles_create_modify_and_delete_with_precise_status() {
        let (dir, r) = repo();
        fs::write(dir.path().join("tracked"), b"original").unwrap();
        r.commit("tester", "base").unwrap();
        fs::write(dir.path().join("tracked"), b"prepared").unwrap();
        fs::write(dir.path().join("new"), b"new").unwrap();
        r.add("tracked").unwrap();
        r.add("new").unwrap();
        assert_eq!(r.staged_status().unwrap(), vec![
            StatusEntry { path: "new".into(), kind: StatusKind::Added },
            StatusEntry { path: "tracked".into(), kind: StatusKind::Modified },
        ]);
        r.commit("tester", "both").unwrap();
        assert!(r.staged_status().unwrap().is_empty());
        fs::remove_file(dir.path().join("tracked")).unwrap();
        r.add("tracked").unwrap();
        assert_eq!(r.staged_status().unwrap(), vec![
            StatusEntry { path: "tracked".into(), kind: StatusKind::Deleted }
        ]);
        r.commit("tester", "delete").unwrap();
        assert_eq!(r.read_branch_file("main", "tracked").unwrap(), None);
        assert!(r.fsck().is_ok());
    }

    #[test]
    fn indexed_add_rebuilds_evicted_and_stale_branch_data() {
        let (dir, r) = repo();
        fs::write(dir.path().join("one"), b"first").unwrap();
        r.commit("tester", "base").unwrap();
        fs::write(dir.path().join("one"), b"second").unwrap();
        r.add("one").unwrap();
        r.commit("tester", "second").unwrap();
        r.db.execute("DELETE FROM branch_path_index_tip", []).unwrap();
        r.db.execute("DELETE FROM branch_path_index", []).unwrap();
        fs::write(dir.path().join("one"), b"third").unwrap();
        r.add("one").unwrap();
        assert_eq!(r.staged_status().unwrap(), vec![
            StatusEntry { path: "one".into(), kind: StatusKind::Modified }
        ]);
        r.commit("tester", "third").unwrap();
        r.db.execute(
            "UPDATE branch_path_index_tip SET oid=?1 WHERE branch='main'",
            [ObjectId([4; 32]).to_string()]
        ).unwrap();
        fs::remove_file(dir.path().join("one")).unwrap();
        r.add("one").unwrap();
        assert_eq!(r.staged_status().unwrap(), vec![
            StatusEntry { path: "one".into(), kind: StatusKind::Deleted }
        ]);
        assert!(r.fsck().is_ok());
    }

    #[test]
    fn staged_status_falls_back_to_authoritative_history_for_legacy_tip() {
        let (dir, r) = repo();
        fs::write(dir.path().join("old"), b"original").unwrap();
        r.commit("tester", "base").unwrap();
        fs::write(dir.path().join("old"), b"changed").unwrap();
        r.add("old").unwrap();
        r.db.execute("DELETE FROM branch_path_index_tip", []).unwrap();
        assert_eq!(r.staged_status().unwrap(), vec![
            StatusEntry { path: "old".into(), kind: StatusKind::Modified }
        ]);
        assert!(r.fsck().is_ok());
    }


    #[test]
    fn historical_file_queries_follow_first_parent_deltas_and_deletions() {
        let (dir, r) = repo();
        fs::write(dir.path().join("a"), b"one").unwrap();
        fs::write(dir.path().join("untouched"), b"stable").unwrap();
        let first = r.commit("tester", "base").unwrap();
        fs::write(dir.path().join("a"), b"two").unwrap();
        let second = r.commit("tester", "update").unwrap();
        fs::remove_file(dir.path().join("a")).unwrap();
        let deleted = r.commit("tester", "remove").unwrap();

        for _ in 0..2 {
            assert_eq!(r.read_revision_file(first, "a").unwrap(),
                       Some(b"one".to_vec()));
            assert_eq!(r.read_revision_file(second, "a").unwrap(),
                       Some(b"two".to_vec()));
            assert_eq!(r.read_revision_file(deleted, "a").unwrap(), None);
            assert_eq!(r.read_revision_file(deleted, "untouched").unwrap(),
                       Some(b"stable".to_vec()));
            assert_eq!(r.read_revision_file(first, "absent").unwrap(), None);
        }
        assert!(r.read_revision_file(ObjectId([9; 32]), "a").is_err());
        assert!(r.read_revision_file(first, "../unsafe").is_err());
        let count: i64 = r.db.query_row(
            "SELECT COUNT(*) FROM revision_file_cache",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 5, "cache records hits and negative lookups");
        assert!(r.fsck().is_ok());
    }

    #[test]
    fn historical_lookup_cache_is_rebuildable_and_fsck_detects_corruption() {
        let (dir, r) = repo();
        fs::write(dir.path().join("one"), b"first").unwrap();
        fs::write(dir.path().join("two"), b"second").unwrap();
        let revision = r.commit("tester", "base").unwrap();
        assert_eq!(r.read_revision_file(revision, "one").unwrap(),
                   Some(b"first".to_vec()));
        r.db.execute("DELETE FROM revision_file_cache", []).unwrap();
        assert_eq!(r.read_revision_file(revision, "one").unwrap(),
                   Some(b"first".to_vec()));
        assert!(r.fsck().is_ok());
        let incorrect = r.state_at(Some(revision)).unwrap()["two"];
        r.db.execute(
            "UPDATE revision_file_cache SET blob_oid=?1
             WHERE revision_oid=?2 AND path='one'",
            params![incorrect.to_string(), revision.to_string()],
        ).unwrap();
        assert!(r.fsck().is_err(),
                "fsck must reject incorrect but otherwise valid cached content");
    }

    #[test]
    fn historical_path_queries_preserve_binary_bytes() {
        let (dir, r) = repo();
        fs::write(dir.path().join("binary"), [0u8, 1, 255, 0, 7]).unwrap();
        let initial = r.commit("tester", "binary").unwrap();
        assert_eq!(r.read_revision_file(initial, "binary").unwrap(),
                   Some(vec![0, 1, 255, 0, 7]));
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
