use loom_core::memory_storage::MemoryObjectBackend;
use loom_core::storage_backend::{ObjectBackend, StoreCapabilities};
use loom_core::{Error, FilesystemObjectBackend, ObjectId, Repository, Result};
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

struct Controlled {
    inner: FilesystemObjectBackend,
    fail: AtomicBool,
    corrupt: AtomicBool,
    writes: AtomicUsize,
}
impl Controlled {
    fn open(path: &std::path::Path) -> Self {
        Self {
            inner: FilesystemObjectBackend::open(path).unwrap(),
            fail: AtomicBool::new(false),
            corrupt: AtomicBool::new(false),
            writes: AtomicUsize::new(0),
        }
    }
}
impl ObjectBackend for Controlled {
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }
    fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
        let mut bytes = self.inner.get(id)?;
        if self.corrupt.load(Ordering::SeqCst) {
            bytes[0] ^= 1;
        }
        Ok(bytes)
    }
    fn put_if_absent(&self, id: ObjectId, bytes: &[u8]) -> Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(Error::Io(std::io::Error::other("injected storage outage")));
        }
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.put_if_absent(id, bytes)
    }
    fn exists(&self, id: ObjectId) -> Result<bool> {
        self.inner.exists(id)
    }
    fn list_ids(&self) -> Result<Vec<ObjectId>> {
        self.inner.list_ids()
    }
}

#[test]
fn external_provider_handles_history_staging_transitions_and_reopen() {
    let workspace = tempfile::tempdir().unwrap();
    let objects = tempfile::tempdir().unwrap();
    let backend = Arc::new(Controlled::open(objects.path()));
    let repo = Repository::init_with_object_backend(workspace.path(), backend.clone()).unwrap();
    fs::write(workspace.path().join("file"), b"base").unwrap();
    let base = repo.commit("test", "base").unwrap();
    repo.create_branch("feature").unwrap();
    repo.switch_branch("feature").unwrap();
    fs::write(workspace.path().join("file"), b"staged").unwrap();
    repo.add("file").unwrap();
    fs::write(workspace.path().join("file"), b"later edits").unwrap();
    let tip = repo.commit("test", "staged only").unwrap();
    assert_eq!(
        repo.read_revision_file(base, "file").unwrap().unwrap(),
        b"base"
    );
    assert_eq!(
        repo.read_revision_file(tip, "file").unwrap().unwrap(),
        b"staged"
    );
    fs::write(workspace.path().join("file"), b"staged").unwrap();
    repo.switch_branch("main").unwrap();
    assert_eq!(repo.merge_branch("feature", "test", "ff").unwrap(), tip);
    assert_eq!(fs::read(workspace.path().join("file")).unwrap(), b"staged");
    assert!(repo.fsck().unwrap() > 0);
    assert!(backend.writes.load(Ordering::SeqCst) > 0);
    assert_eq!(
        fs::read_dir(workspace.path().join(".loom/objects"))
            .unwrap()
            .count(),
        0
    );
    drop(repo);
    assert!(Repository::discover(workspace.path()).is_err());
    let repo = Repository::discover_with_object_backend(workspace.path(), backend).unwrap();
    assert_eq!(repo.head().unwrap(), Some(tip));
    assert!(repo.fsck().unwrap() > 0);
    let wrong = tempfile::tempdir().unwrap();
    assert!(Repository::discover_with_object_backend(
        workspace.path(),
        Arc::new(FilesystemObjectBackend::open(wrong.path()).unwrap()),
    )
    .is_err());
    assert_eq!(fs::read(workspace.path().join("file")).unwrap(), b"staged");
}

#[test]
fn failed_or_corrupt_provider_never_advances_branch() {
    let workspace = tempfile::tempdir().unwrap();
    let objects = tempfile::tempdir().unwrap();
    let backend = Arc::new(Controlled::open(objects.path()));
    let repo = Repository::init_with_object_backend(workspace.path(), backend.clone()).unwrap();
    fs::write(workspace.path().join("file"), b"base").unwrap();
    let base = repo.commit("test", "base").unwrap();
    fs::write(workspace.path().join("file"), b"changed").unwrap();
    repo.add("file").unwrap();
    backend.fail.store(true, Ordering::SeqCst);
    assert!(repo.commit("test", "fails").is_err());
    assert_eq!(repo.head().unwrap(), Some(base));
    assert!(!repo.staged_status().unwrap().is_empty());
    backend.fail.store(false, Ordering::SeqCst);
    backend.corrupt.store(true, Ordering::SeqCst);
    assert!(repo.commit("test", "corrupt").is_err());
    assert!(repo.read_revision_file(base, "file").is_err());
    assert_eq!(repo.head().unwrap(), Some(base));
    backend.corrupt.store(false, Ordering::SeqCst);
    let next = repo.commit("test", "retry").unwrap();
    assert_ne!(next, base);
    assert!(repo.fsck().is_ok());
}

#[test]
fn volatile_providers_and_silent_provider_switches_are_rejected() {
    let workspace = tempfile::tempdir().unwrap();
    assert!(Repository::init_with_object_backend(
        workspace.path(),
        Arc::new(MemoryObjectBackend::new()),
    )
    .is_err());
    assert!(!workspace.path().join(".loom").exists());
    let local = Repository::init(workspace.path()).unwrap();
    let objects = tempfile::tempdir().unwrap();
    assert!(Repository::discover_with_object_backend(
        workspace.path(),
        Arc::new(FilesystemObjectBackend::open(objects.path()).unwrap()),
    )
    .is_err());
    assert_eq!(local.head().unwrap(), None);
}

#[test]
fn missing_historical_blobs_block_reopen_and_ref_publication() {
    let workspace = tempfile::tempdir().unwrap();
    let objects = tempfile::tempdir().unwrap();
    let backend = Arc::new(Controlled::open(objects.path()));
    let repo = Repository::init_with_object_backend(workspace.path(), backend.clone()).unwrap();
    fs::write(workspace.path().join("file"), b"old bytes").unwrap();
    repo.commit("test", "base").unwrap();
    let old_blob = repo.log(1).unwrap()[0].1.changes[0].blob.unwrap();
    fs::write(workspace.path().join("file"), b"new bytes").unwrap();
    let tip = repo.commit("test", "changed").unwrap();
    let hex = old_blob.to_string();
    fs::remove_file(objects.path().join(&hex[..2]).join(&hex[2..])).unwrap();
    // Current file bytes remain present. The old immutable history is incomplete.
    assert_eq!(
        repo.read_branch_file("main", "file").unwrap().unwrap(),
        b"new bytes"
    );
    assert!(repo.create_branch("must-not-publish").is_err());
    fs::write(workspace.path().join("other"), b"more bytes").unwrap();
    assert!(repo.commit("test", "must fail").is_err());
    assert_eq!(repo.head().unwrap(), Some(tip));
    drop(repo);
    assert!(Repository::discover_with_object_backend(workspace.path(), backend).is_err());
    assert_eq!(
        fs::read(workspace.path().join("file")).unwrap(),
        b"new bytes"
    );
}

#[test]
fn replicated_repository_requires_history_on_every_replica() {
    use loom_core::replicated_storage::ReplicatedObjectBackend;
    let workspace = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let provider = Arc::new(
        ReplicatedObjectBackend::new(vec![
            Arc::new(FilesystemObjectBackend::open(first.path()).unwrap()),
            Arc::new(FilesystemObjectBackend::open(second.path()).unwrap()),
        ])
        .unwrap(),
    );
    let repo = Repository::init_with_object_backend(workspace.path(), provider.clone()).unwrap();
    fs::write(workspace.path().join("file"), b"replicated").unwrap();
    let tip = repo.commit("test", "both replicas").unwrap();
    // A non-listing provider must not claim a full-store fsck was completed.
    assert!(repo.fsck().unwrap_err().to_string().contains("enumeration"));
    let blob = repo.log(1).unwrap()[0].1.changes[0].blob.unwrap();
    let hex = blob.to_string();
    fs::remove_file(second.path().join(&hex[..2]).join(&hex[2..])).unwrap();
    // The primary can still read it, but a new ref cannot promise full replication.
    assert_eq!(
        repo.read_branch_file("main", "file").unwrap().unwrap(),
        b"replicated"
    );
    assert!(repo.create_branch("incomplete").is_err());
    assert_eq!(repo.head().unwrap(), Some(tip));
    drop(repo);
    assert!(Repository::discover_with_object_backend(workspace.path(), provider).is_err());
}

#[test]
fn filesystem_and_sqlite_replicas_preserve_identical_native_history() {
    use loom_core::replicated_storage::ReplicatedObjectBackend;
    use loom_core::sqlite_object_storage::SqliteObjectBackend;
    let workspace = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let database = tempfile::tempdir().unwrap();
    let sqlite =
        Arc::new(SqliteObjectBackend::open(database.path().join("objects.sqlite")).unwrap());
    let filesystem = Arc::new(FilesystemObjectBackend::open(files.path()).unwrap());
    let replicas =
        Arc::new(ReplicatedObjectBackend::new(vec![filesystem.clone(), sqlite.clone()]).unwrap());
    let repo = Repository::init_with_object_backend(workspace.path(), replicas).unwrap();
    fs::write(workspace.path().join("file"), b"cross provider").unwrap();
    let tip = repo.commit("test", "one identity").unwrap();
    let ids = filesystem.list_ids().unwrap();
    assert!(!ids.is_empty());
    assert_eq!(ids.len(), sqlite.list_ids().unwrap().len());
    for id in ids {
        assert_eq!(filesystem.get(id).unwrap(), sqlite.get(id).unwrap());
    }
    drop(repo);
    // Either complete provider independently reconstructs the same repository.
    let repo = Repository::discover_with_object_backend(workspace.path(), sqlite).unwrap();
    assert_eq!(repo.head().unwrap(), Some(tip));
    assert_eq!(
        repo.read_branch_file("main", "file").unwrap().unwrap(),
        b"cross provider"
    );
    assert!(repo.fsck().unwrap() > 0);
}

#[test]
fn failed_parallel_publication_leaves_refs_unchanged_and_retry_is_lossless() {
    struct Failing {
        inner: FilesystemObjectBackend,
        calls: AtomicUsize,
        fail_after: AtomicUsize,
        active: AtomicUsize,
    }
    struct Active<'a>(&'a AtomicUsize);
    impl Drop for Active<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl ObjectBackend for Failing {
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
            4
        }
        fn put_if_absent(&self, id: ObjectId, bytes: &[u8]) -> Result<()> {
            self.active.fetch_add(1, Ordering::SeqCst);
            let _active = Active(&self.active);
            if self.calls.fetch_add(1, Ordering::SeqCst) >= self.fail_after.load(Ordering::SeqCst) {
                return Err(Error::Io(std::io::Error::other("injected window failure")));
            }
            self.inner.put_if_absent(id, bytes)
        }
    }
    let workspace = tempfile::tempdir().unwrap();
    let objects = tempfile::tempdir().unwrap();
    let backend = Arc::new(Failing {
        inner: FilesystemObjectBackend::open(objects.path()).unwrap(),
        calls: AtomicUsize::new(0),
        fail_after: AtomicUsize::new(usize::MAX),
        active: AtomicUsize::new(0),
    });
    let repo = Repository::init_with_object_backend(workspace.path(), backend.clone()).unwrap();
    fs::write(workspace.path().join("seed"), b"baseline").unwrap();
    let base = repo.commit("test", "base").unwrap();
    for n in 0..300 {
        let folder = workspace.path().join(format!("dir{:02}", n % 32));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(format!("file{n:03}")), format!("value {n}")).unwrap();
    }
    backend
        .fail_after
        .store(backend.calls.load(Ordering::SeqCst) + 130, Ordering::SeqCst);
    assert!(repo.commit("test", "must not publish").is_err());
    assert_eq!(backend.active.load(Ordering::SeqCst), 0);
    assert_eq!(repo.head().unwrap(), Some(base));
    let db = rusqlite::Connection::open(workspace.path().join(".loom/index.sqlite")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM revision_native_tree", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        repo.read_revision_file(base, "seed").unwrap().unwrap(),
        b"baseline"
    );
    backend.fail_after.store(usize::MAX, Ordering::SeqCst);
    let tip = repo.commit("test", "retry").unwrap();
    assert_eq!(repo.head().unwrap(), Some(tip));
    let snapshot = repo.snapshot(tip).unwrap();
    for n in 0..300 {
        let path = format!("dir{:02}/file{n:03}", n % 32);
        assert_eq!(
            snapshot.read_file(&path).unwrap().unwrap().as_ref(),
            format!("value {n}").as_bytes()
        );
    }
    assert_eq!(repo.log(10).unwrap().len(), 2);
    assert!(repo.fsck().is_ok());
    assert_eq!(backend.active.load(Ordering::SeqCst), 0);
}
