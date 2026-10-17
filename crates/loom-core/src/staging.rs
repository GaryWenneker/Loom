//! Atomic staging of explicit path sets. This is a child of repository so the
//! metadata connection and publication capabilities stay private to that module.
use super::Repository;
use crate::format::validate_path;
use crate::{Error, Result, WorktreeScanOptions};
use rusqlite::{params, OptionalExtension, Params};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

const MAX_PATHS: usize = 1_000_000;

fn insert(paths: &mut BTreeSet<String>, path: String) -> Result<()> {
    paths.insert(path);
    if paths.len() > MAX_PATHS {
        return Err(Error::Invalid("too many selected paths".into()));
    }
    Ok(())
}

fn scopes<P: AsRef<str>>(paths: &[P]) -> Result<(Vec<String>, BTreeSet<String>)> {
    if paths.is_empty() || paths.len() > MAX_PATHS {
        return Err(Error::Invalid(
            "select 1..1000000 paths or directories".into(),
        ));
    }
    let mut normalized = BTreeSet::new();
    for path in paths {
        let raw = path.as_ref();
        let raw = if cfg!(windows) {
            raw.replace('\\', "/")
        } else {
            raw.to_string()
        };
        if raw.is_empty() || raw.starts_with('/') {
            return Err(Error::Invalid("invalid selected path".into()));
        }
        let mut value = raw.as_str();
        while let Some(rest) = value.strip_prefix("./") {
            value = rest;
        }
        let value = value.trim_end_matches('/');
        let value = if value == "." || value.is_empty() {
            ""
        } else {
            validate_path(value)?;
            value
        };
        normalized.insert(value.to_string());
    }
    let mut result: Vec<String> = Vec::new();
    let mut parents = BTreeSet::new();
    for value in &normalized {
        let covered = parents.contains("")
            || value
                .char_indices()
                .any(|(index, ch)| ch == '/' && parents.contains(&value[..index]));
        if !covered {
            parents.insert(value.as_str());
            result.push(value.clone());
        }
    }
    Ok((result, normalized))
}

fn collect_files(root: &Path, scope: &str, paths: &mut BTreeSet<String>) -> Result<bool> {
    let start = if scope.is_empty() {
        root.to_path_buf()
    } else {
        match crate::worktree_scan::inspect_selected(root, scope)? {
            None => return Ok(false),
            Some((_, metadata)) if metadata.is_file() => {
                insert(paths, scope.to_string())?;
                return Ok(true);
            }
            Some((absolute, _)) => absolute,
        }
    };
    let mut stack = vec![start];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_name() == ".loom" || entry.file_name() == ".git" {
                continue;
            }
            let absolute = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(absolute);
                continue;
            }
            let relative = absolute
                .strip_prefix(root)
                .map_err(|_| Error::Invalid("selected path escaped worktree".into()))?;
            let value = relative
                .to_str()
                .ok_or_else(|| Error::Invalid("non-UTF-8 path not supported in v1".into()))?;
            let path = if cfg!(windows) {
                value.replace('\\', "/")
            } else {
                value.to_string()
            };
            validate_path(&path)?;
            insert(paths, path)?;
        }
    }
    Ok(true)
}

impl Repository {
    fn collect_stage_query(
        &self,
        sql: &str,
        values: impl Params,
        paths: &mut BTreeSet<String>,
    ) -> Result<()> {
        let mut query = self.db.prepare(sql)?;
        for path in query.query_map(values, |row| row.get::<_, String>(0))? {
            insert(paths, path?)?;
        }
        Ok(())
    }

    fn collect_scope_index(
        &self,
        branch: &str,
        scope: &str,
        paths: &mut BTreeSet<String>,
        staged: &mut BTreeSet<String>,
    ) -> Result<()> {
        if scope.is_empty() {
            self.collect_stage_query(
                "SELECT path FROM branch_path_index WHERE branch=?1",
                [branch],
                paths,
            )?;
            self.collect_stage_query("SELECT path FROM stage_index", [], staged)?;
        } else {
            let lower = format!("{scope}/");
            let upper = format!("{scope}0");
            self.collect_stage_query(
                "SELECT path FROM branch_path_index WHERE branch=?1 AND path=?2",
                params![branch, scope],
                paths,
            )?;
            self.collect_stage_query(
                "SELECT path FROM branch_path_index WHERE branch=?1 AND path>=?2 AND path<?3",
                params![branch, lower, upper],
                paths,
            )?;
            self.collect_stage_query(
                "SELECT path FROM stage_index WHERE path=?1",
                [scope],
                staged,
            )?;
            self.collect_stage_query(
                "SELECT path FROM stage_index WHERE path>=?1 AND path<?2",
                params![lower, upper],
                staged,
            )?;
        }
        Ok(())
    }

    pub fn add(&self, path: &str) -> Result<()> {
        self.add_paths(&[path]).map(|_| ())
    }

    /// Atomically capture files/directories in one selected set. Root `.` is
    /// explicit full selection. Missing tracked descendants become deletions;
    /// paths outside the selection are neither read nor unstaged. Returns the
    /// number of selected candidate paths, including unchanged paths.
    pub fn add_paths<P: AsRef<str>>(&self, paths: &[P]) -> Result<usize> {
        self.add_paths_with_scan_options(paths, WorktreeScanOptions::default())
    }

    pub fn add_paths_with_scan_options<P: AsRef<str>>(
        &self,
        paths: &[P],
        options: WorktreeScanOptions,
    ) -> Result<usize> {
        let (scopes, explicit) = scopes(paths)?;
        let roots: BTreeSet<_> = scopes.iter().map(String::as_str).collect();
        let _guard = self.lock_worktree()?;
        self.recover_locked()?;
        let branch = self.current_branch()?;
        let tip = self.branch_tip(&branch)?;
        self.ensure_branch_path_index(&branch, tip)?;
        let ignores = crate::ignore::IgnoreRules::load(&self.root)?;
        let mut candidates = BTreeSet::new();
        let mut staged = BTreeSet::new();
        for scope in &scopes {
            let mut tracked = BTreeSet::new();
            let mut prepared = BTreeSet::new();
            self.collect_scope_index(&branch, scope, &mut tracked, &mut prepared)?;
            let present = collect_files(&self.root, scope, &mut candidates)?;
            if !present && tracked.is_empty() && prepared.is_empty() {
                return Err(Error::Invalid(format!(
                    "path is neither tracked nor present: {scope}"
                )));
            }
            for path in tracked {
                insert(&mut candidates, path)?;
            }
            for path in prepared {
                insert(&mut candidates, path.clone())?;
                staged.insert(path);
            }
        }
        for scope in explicit
            .iter()
            .filter(|scope| !roots.contains(scope.as_str()))
        {
            if crate::worktree_scan::inspect_selected(&self.root, scope)?.is_none()
                && !candidates.contains(scope)
                && candidates
                    .range(format!("{scope}/")..format!("{scope}0"))
                    .next()
                    .is_none()
            {
                return Err(Error::Invalid(format!(
                    "path is neither tracked nor present: {scope}"
                )));
            }
        }
        let mut selected = BTreeMap::new();
        {
            let mut query = self
                .db
                .prepare("SELECT oid FROM branch_path_index WHERE branch=?1 AND path=?2")?;
            for path in candidates {
                let raw: Option<String> = query
                    .query_row(params![branch, path], |row| row.get(0))
                    .optional()?;
                let prior = raw
                    .map(|value| crate::ObjectId::parse(&value))
                    .transpose()?;
                if prior.is_none() && !staged.contains(&path) && ignores.ignores(&path) {
                    if explicit.contains(&path) {
                        return Err(Error::Invalid(format!("ignored untracked path: {path}")));
                    }
                    continue;
                }
                selected.insert(path, prior);
            }
        }
        let mut publication = self.store.publisher();
        let changes =
            crate::worktree_scan::selected_changes(&self.root, &selected, options, |id, bytes| {
                publication.push(id, bytes)
            })?;
        publication.finish()?;
        let changes: BTreeMap<_, _> = changes
            .into_iter()
            .map(|change| (change.path, change.blob))
            .collect();
        let tx = self.db.unchecked_transaction()?;
        let head: String = tx.query_row("SELECT value FROM meta WHERE key='head'", [], |row| {
            row.get(0)
        })?;
        let current: Option<Option<String>> = tx
            .query_row("SELECT oid FROM refs WHERE name=?1", [&branch], |row| {
                row.get(0)
            })
            .optional()?;
        if head != branch || current != Some(tip.map(|id| id.to_string())) {
            return Err(Error::Conflict(
                "branch moved while preparing selected paths".into(),
            ));
        }
        tx.execute("INSERT INTO meta(key,value) VALUES('staging_mode','enabled') ON CONFLICT(key) DO UPDATE SET value='enabled'", [])?;
        {
            let mut remove = tx.prepare("DELETE FROM stage_index WHERE path=?1")?;
            let mut write = tx.prepare("INSERT INTO stage_index(path,blob_id) VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET blob_id=excluded.blob_id")?;
            for path in selected.keys() {
                if let Some(blob) = changes.get(path) {
                    write.execute(params![path, blob.map(|id| id.to_string())])?;
                } else {
                    remove.execute([path])?;
                }
            }
        }
        tx.commit()?;
        Ok(selected.len())
    }

    pub fn unstage(&self, path: &str) -> Result<()> {
        self.unstage_paths(&[path]).map(|_| ())
    }

    /// Atomically remove selected files/subtrees from staging without reading
    /// worktree payloads. Explicit staging mode remains enabled.
    pub fn unstage_paths<P: AsRef<str>>(&self, paths: &[P]) -> Result<usize> {
        let (scopes, _) = scopes(paths)?;
        let _guard = self.lock_worktree()?;
        self.recover_locked()?;
        let tx = self.db.unchecked_transaction()?;
        let mut removed = 0;
        for scope in scopes {
            if scope.is_empty() {
                removed += tx.execute("DELETE FROM stage_index", [])?;
            } else {
                removed += tx.execute("DELETE FROM stage_index WHERE path=?1", [&scope])?;
                removed += tx.execute(
                    "DELETE FROM stage_index WHERE path>=?1 AND path<?2",
                    params![format!("{scope}/"), format!("{scope}0")],
                )?;
            }
        }
        tx.commit()?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, bytes: &[u8]) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn large_explicit_sets_do_not_require_a_directory_workaround() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        let paths: Vec<_> = (0..5000).map(|n| format!("file-{n:05}")).collect();
        for path in &paths {
            fs::write(temp.path().join(path), []).unwrap();
        }
        assert_eq!(repo.add_paths(&paths).unwrap(), paths.len());
        assert_eq!(repo.staged_status().unwrap().len(), paths.len());
    }

    #[test]
    fn directory_sets_capture_deletions_and_keep_other_prefixes_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let repo = Repository::init(root).unwrap();
        for path in ["src/a", "src/deleted", "src-other/a", "docs/a"] {
            write(root, path, b"base");
        }
        let base = repo.commit("test", "base").unwrap();
        write(root, "src/a", b"selected");
        write(root, "src/new", b"new\0\xff");
        write(root, "src-other/a", b"outside");
        write(root, "docs/a", b"unselected");
        fs::remove_file(root.join("src/deleted")).unwrap();
        fs::File::create(root.join("outside.big"))
            .unwrap()
            .set_len(65 * 1024 * 1024)
            .unwrap();
        assert_eq!(repo.add_paths(&["src/", "./src/a", "src"]).unwrap(), 3);
        assert_eq!(repo.staged_status().unwrap().len(), 3);
        write(root, "src/a", b"after preparation");
        let tip = repo.commit("test", "selected set").unwrap();
        assert_eq!(
            repo.read_revision_file(tip, "src/a").unwrap(),
            Some(b"selected".to_vec())
        );
        assert_eq!(
            repo.read_revision_file(tip, "src/new").unwrap(),
            Some(b"new\0\xff".to_vec())
        );
        assert_eq!(repo.read_revision_file(tip, "src/deleted").unwrap(), None);
        assert_eq!(
            repo.read_revision_file(tip, "src-other/a").unwrap(),
            Some(b"base".to_vec())
        );
        assert_eq!(
            repo.read_revision_file(tip, "docs/a").unwrap(),
            Some(b"base".to_vec())
        );
        assert_eq!(
            repo.read_revision_file(base, "src/a").unwrap(),
            Some(b"base".to_vec())
        );
        repo.fsck().unwrap();
    }

    #[test]
    fn batch_errors_retain_all_previous_staged_bytes_and_mode() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        write(temp.path(), "a", b"first");
        write(temp.path(), "b", b"second");
        assert!(repo.add_paths(&["a", "missing"]).is_err());
        assert!(!repo.staging_enabled().unwrap());
        assert!(repo.staged_status().unwrap().is_empty());
        repo.add("a").unwrap();
        let before = repo.diff_staged().unwrap();
        write(temp.path(), "a", b"later");
        for paths in [
            vec!["a", "missing"],
            vec!["b", "../escape"],
            vec![".", "missing"],
        ] {
            assert!(repo.add_paths(&paths).is_err());
            assert_eq!(repo.diff_staged().unwrap(), before);
        }
        assert!(repo.unstage_paths(&["a", "../escape"]).is_err());
        assert_eq!(repo.diff_staged().unwrap(), before);
    }

    #[test]
    fn sql_failure_rolls_back_every_path_and_staging_activation() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        write(temp.path(), "a", b"first");
        write(temp.path(), "z", b"last");
        repo.db.execute_batch("CREATE TRIGGER reject_last BEFORE INSERT ON stage_index WHEN NEW.path='z' BEGIN SELECT RAISE(ABORT,'injected stage failure'); END;").unwrap();
        assert!(repo.add_paths(&["a", "z"]).is_err());
        assert!(!repo.staging_enabled().unwrap());
        assert!(repo.staged_status().unwrap().is_empty());
        assert_eq!(repo.head().unwrap(), None);
        repo.db.execute_batch("DROP TRIGGER reject_last").unwrap();
        repo.add_paths(&["a", "z"]).unwrap();
        repo.commit("test", "retry").unwrap();
        repo.fsck().unwrap();
    }

    #[test]
    fn root_selection_honors_ignores_and_prepared_untracked_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let repo = Repository::init(root).unwrap();
        write(root, "tracked.tmp", b"base");
        repo.commit("test", "base").unwrap();
        write(root, "prepared.tmp", b"prepared");
        repo.add("prepared.tmp").unwrap();
        write(root, ".loomignore", b"*.tmp\n");
        write(root, "tracked.tmp", b"changed");
        write(root, "ignored.tmp", b"ignored");
        write(root, ".git/secret", b"metadata");
        assert!(repo.add_paths(&[".", "ignored.tmp"]).is_err());
        repo.add_paths(&[".", "./"]).unwrap();
        let paths: Vec<_> = repo
            .staged_status()
            .unwrap()
            .into_iter()
            .map(|s| s.path)
            .collect();
        assert_eq!(paths, vec![".loomignore", "prepared.tmp", "tracked.tmp"]);
        fs::remove_file(root.join("prepared.tmp")).unwrap();
        repo.add_paths(&["prepared.tmp"]).unwrap();
        assert!(!repo
            .staged_status()
            .unwrap()
            .iter()
            .any(|s| s.path == "prepared.tmp"));
        let tip = repo.commit("test", "root").unwrap();
        assert_eq!(repo.read_revision_file(tip, "ignored.tmp").unwrap(), None);
        assert_eq!(
            repo.read_revision_file(tip, "tracked.tmp").unwrap(),
            Some(b"changed".to_vec())
        );
        repo.fsck().unwrap();
    }

    #[test]
    fn directory_file_replacements_and_deleted_subtrees_are_complete() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let repo = Repository::init(root).unwrap();
        write(root, "node", b"file");
        repo.commit("test", "file").unwrap();
        fs::remove_file(root.join("node")).unwrap();
        write(root, "node/child", b"child");
        repo.add_paths(&["node"]).unwrap();
        let directory = repo.commit("test", "directory").unwrap();
        assert_eq!(
            repo.read_revision_file(directory, "node/child").unwrap(),
            Some(b"child".to_vec())
        );
        fs::remove_dir_all(root.join("node")).unwrap();
        write(root, "node", b"replacement");
        repo.add_paths(&["node"]).unwrap();
        let file = repo.commit("test", "file again").unwrap();
        assert_eq!(repo.read_revision_file(file, "node/child").unwrap(), None);
        assert_eq!(
            repo.read_revision_file(file, "node").unwrap(),
            Some(b"replacement".to_vec())
        );
        fs::remove_file(root.join("node")).unwrap();
        repo.add_paths(&["node"]).unwrap();
        repo.commit("test", "deleted").unwrap();
        repo.fsck().unwrap();
    }

    #[test]
    fn unstage_subtrees_uses_exact_prefix_boundaries_without_worktree_reads() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let repo = Repository::init(root).unwrap();
        for path in ["src/a", "src0/a", "src-other/a"] {
            write(root, path, b"bytes");
        }
        repo.add_paths(&["."]).unwrap();
        fs::remove_dir_all(root.join("src")).unwrap();
        assert_eq!(repo.unstage_paths(&["src/", "src/a"]).unwrap(), 1);
        assert_eq!(repo.staged_status().unwrap().len(), 2);
        assert_eq!(repo.unstage_paths(&["."]).unwrap(), 2);
        assert!(repo.staging_enabled().unwrap());
        assert!(repo.commit("test", "requires explicit staging").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_in_selected_directory_does_not_partially_replace_staging() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        write(temp.path(), "existing", b"frozen");
        repo.add("existing").unwrap();
        let before = repo.diff_staged().unwrap();
        write(temp.path(), "chosen/good", b"valid");
        write(outside.path(), "secret", b"outside");
        std::os::unix::fs::symlink(
            outside.path().join("secret"),
            temp.path().join("chosen/link"),
        )
        .unwrap();
        assert!(repo.add_paths(&["chosen"]).is_err());
        assert_eq!(repo.diff_staged().unwrap(), before);
    }
}
