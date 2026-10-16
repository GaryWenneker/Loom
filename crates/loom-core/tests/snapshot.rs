use loom_core::Repository;
use std::{fs, sync::Arc};

fn fixture() -> (tempfile::TempDir, Repository, loom_core::ObjectId) {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    fs::create_dir(dir.path().join("a")).unwrap();
    fs::write(dir.path().join("a/file"), b"nested\0bytes\xff").unwrap();
    fs::write(dir.path().join("a.txt"), b"first").unwrap();
    fs::write(dir.path().join("z"), b"last").unwrap();
    let revision = repo.commit("test", "base").unwrap();
    (dir, repo, revision)
}

#[test]
fn read_pack_supports_old_and_new_revisions_without_writer_lock_or_cache() {
    let (dir, repo, revision) = fixture();
    let path = dir.path().join(".loom/reads.pack");
    let head = repo.head().unwrap();
    let stats = repo.write_read_pack(revision, &path).unwrap();
    assert_eq!(repo.head().unwrap(), head);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.path().join(".loom/workspace.lock"))
        .unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let query_root = dir.path().to_owned();
    let query_pack = path.clone();
    std::thread::spawn(move || {
        let _ = send.send(Repository::query_snapshot_with_read_pack(
            query_root,
            revision,
            None,
            0,
            Some(&query_pack),
        ));
    });
    let result = receive.recv_timeout(std::time::Duration::from_secs(5));
    fs2::FileExt::unlock(&lock).unwrap();
    let old = result
        .expect("pack query blocked on workspace writer")
        .unwrap();
    assert_eq!(
        &*old.read_file("a/file").unwrap().unwrap(),
        b"nested\0bytes\xff"
    );
    assert_eq!(old.stats().unwrap().cache_bytes, 0);
    assert_eq!(old.stats().unwrap().read_pack_objects, stats.objects);
    assert_eq!(
        old.stats().unwrap().read_pack_index_bytes,
        stats.objects * 48
    );
    fs::write(dir.path().join("a/file"), b"changed").unwrap();
    let next = repo.commit("test", "after pack").unwrap();
    let new =
        Repository::query_snapshot_with_read_pack(dir.path(), next, None, 0, Some(&path)).unwrap();
    let result = new
        .read_files(&["a/file", "a.txt", "missing", "a/file"], 100)
        .unwrap();
    assert_eq!(result[0].as_deref(), Some(b"changed".as_slice()));
    assert_eq!(result[1].as_deref(), Some(b"first".as_slice()));
    assert!(result[2].is_none());
    assert_eq!(result[0], result[3]);
    assert_eq!(
        &*old.read_file("a/file").unwrap().unwrap(),
        b"nested\0bytes\xff"
    );
    assert!(repo.fsck().is_ok());
    fs::remove_file(&path).unwrap();
    // Dropping an optional accelerator never loses repository data.
    assert_eq!(
        &*Repository::query_snapshot(dir.path(), next, None, 0)
            .unwrap()
            .read_file("a/file")
            .unwrap()
            .unwrap(),
        b"changed"
    );
}

#[test]
fn read_pack_works_with_sqlite_objects_and_keeps_live_integrity_checks() {
    let dir = tempfile::tempdir().unwrap();
    let objects = tempfile::tempdir().unwrap();
    let backend = Arc::new(
        loom_core::sqlite_object_storage::SqliteObjectBackend::open(
            objects.path().join("objects.sqlite"),
        )
        .unwrap(),
    );
    let repo = Repository::init_with_object_backend(dir.path(), backend.clone()).unwrap();
    fs::write(dir.path().join("file"), b"SQLite bytes").unwrap();
    let revision = repo.commit("test", "packed SQLite").unwrap();
    let path = dir.path().join(".loom/reads.pack");
    repo.write_read_pack(revision, &path).unwrap();
    let query = Repository::query_snapshot_with_read_pack(
        dir.path(),
        revision,
        Some(backend),
        0,
        Some(&path),
    )
    .unwrap();
    assert_eq!(&*query.read_file("file").unwrap().unwrap(), b"SQLite bytes");
    assert!(repo.fsck().is_ok());
}

#[test]
fn snapshot_is_revision_pinned_and_reuses_verified_decoded_objects() {
    let (dir, repo, revision) = fixture();
    let snapshot = repo.snapshot(revision).unwrap();
    for _ in 0..30 {
        assert_eq!(
            &*snapshot.read_file("a/file").unwrap().unwrap(),
            b"nested\0bytes\xff"
        );
    }
    let stats = snapshot.stats().unwrap();
    assert_eq!(stats.tree_reads, 2);
    assert_eq!(stats.blob_reads, 1);
    assert!(stats.path_cache_hits >= 29);
    fs::write(dir.path().join("a/file"), b"changed").unwrap();
    let next = repo.commit("test", "advance branch").unwrap();
    assert_ne!(next, revision);
    assert_eq!(
        &*snapshot.read_file("a/file").unwrap().unwrap(),
        b"nested\0bytes\xff"
    );
    assert_eq!(
        &*repo
            .snapshot(next)
            .unwrap()
            .read_file("a/file")
            .unwrap()
            .unwrap(),
        b"changed"
    );
    assert!(repo.fsck().is_ok());
}

#[test]
fn working_set_cache_preserves_partial_hits_missing_slots_and_byte_limits() {
    let (_dir, repo, revision) = fixture();
    let query = repo.snapshot(revision).unwrap();
    assert_eq!(
        query.read_file("a.txt").unwrap().as_deref(),
        Some(b"first".as_slice())
    );
    assert!(query.read_file("missing").unwrap().is_none());
    let paths = ["missing", "a.txt", "a/file", "a.txt", "a"];
    let first = query.read_files(&paths, 100).unwrap();
    assert!(first[0].is_none());
    assert_eq!(first[1].as_deref(), Some(b"first".as_slice()));
    assert_eq!(first[2].as_deref(), Some(b"nested\0bytes\xff".as_slice()));
    assert_eq!(first[1], first[3]);
    assert!(first[4].is_none());
    let before = query.stats().unwrap();
    assert_eq!(query.read_files(&paths, 100).unwrap(), first);
    let after = query.stats().unwrap();
    assert_eq!(after.tree_reads, before.tree_reads);
    assert_eq!(after.blob_reads, before.blob_reads);
    assert_eq!(
        after.path_cache_hits - before.path_cache_hits,
        paths.len() as u64
    );
    assert!(query.read_files(&["a.txt", "a.txt"], 9).is_err());
    assert!(query.read_files(&["a.txt", "../invalid"], 100).is_err());
    assert_eq!(
        query.read_files(&["missing", "a"], 0).unwrap(),
        [None, None]
    );
    assert_eq!(query.files(Some("a"), None, 10).unwrap().paths, ["a/file"]);
}

#[test]
fn bulk_input_views_are_captured_once_before_validation_and_memoization() {
    struct ChangingPath {
        calls: std::cell::Cell<usize>,
        later: &'static str,
    }
    impl AsRef<str> for ChangingPath {
        fn as_ref(&self) -> &str {
            let calls = self.calls.get();
            self.calls.set(calls + 1);
            if calls < 4 {
                "a.txt"
            } else {
                self.later
            }
        }
    }
    let (_dir, repo, revision) = fixture();
    for later in ["z", "../invalid"] {
        let query = repo.snapshot(revision).unwrap();
        let path = ChangingPath {
            calls: std::cell::Cell::new(0),
            later,
        };
        assert_eq!(
            query.read_files(std::slice::from_ref(&path), 100).unwrap()[0].as_deref(),
            Some(b"first".as_slice())
        );
        assert_eq!(path.calls.get(), 1);
        assert_eq!(
            query.read_file("z").unwrap().as_deref(),
            Some(b"last".as_slice())
        );
        assert!(query.read_file("../invalid").is_err());
    }
}

#[test]
fn negative_working_set_answers_are_pinned_to_their_own_revision() {
    let (dir, repo, revision) = fixture();
    let old = repo.snapshot(revision).unwrap();
    assert!(old.read_file("later").unwrap().is_none());
    fs::write(dir.path().join("later"), b"new revision").unwrap();
    let next = repo.commit("test", "new path").unwrap();
    assert!(old.read_file("later").unwrap().is_none());
    assert_eq!(
        repo.snapshot(next)
            .unwrap()
            .read_file("later")
            .unwrap()
            .as_deref(),
        Some(b"new revision".as_slice())
    );
}

#[test]
fn mixed_path_and_object_eviction_stays_inside_the_same_budget() {
    let (_dir, repo, revision) = fixture();
    for budget in [0, 512, 2048] {
        let query = repo.snapshot_with_cache_bytes(revision, budget).unwrap();
        for n in 0..100 {
            assert!(query.read_file(&format!("missing-{n}")).unwrap().is_none());
            assert_eq!(
                query.read_file("a.txt").unwrap().as_deref(),
                Some(b"first".as_slice())
            );
            assert_eq!(
                query
                    .read_files(&["a/file", "a.txt", "missing"], 100)
                    .unwrap()[1]
                    .as_deref(),
                Some(b"first".as_slice())
            );
            let stats = query.stats().unwrap();
            assert!(stats.cache_bytes <= budget);
            assert!(stats.path_cache_entries <= stats.cache_entries);
            if budget == 0 {
                assert_eq!(stats.path_cache_hits, 0);
                assert_eq!(stats.cache_entries, 0);
            }
        }
    }
}

#[test]
fn failed_bulk_read_never_publishes_partial_path_answers() {
    let (dir, repo, revision) = fixture();
    let root = repo.native_tree_root(revision).unwrap();
    let id = repo.native_tree_entries(root, Some("a")).unwrap()[0].object;
    let hex = id.to_string();
    let path = dir
        .path()
        .join(".loom/objects")
        .join(&hex[..2])
        .join(&hex[2..]);
    let original = fs::read(&path).unwrap();
    let query = repo.snapshot(revision).unwrap();
    fs::write(&path, b"corrupt").unwrap();
    assert!(query.read_files(&["a.txt", "a/file"], 100).is_err());
    assert_eq!(query.stats().unwrap().path_cache_entries, 0);
    fs::write(&path, original).unwrap();
    assert!(query
        .read_files(&["a.txt", "a/file"], 100)
        .unwrap()
        .iter()
        .all(Option::is_some));
}

#[test]
fn pages_keep_full_path_order_and_component_scoped_prefixes() {
    let (_dir, repo, revision) = fixture();
    let snapshot = repo.snapshot(revision).unwrap();
    let first = snapshot.files(None, None, 1).unwrap();
    assert_eq!(first.paths, ["a.txt"]);
    assert_eq!(first.next_cursor.as_deref(), Some("a.txt"));
    let rest = snapshot
        .files(None, first.next_cursor.as_deref(), 10)
        .unwrap();
    assert_eq!(rest.paths, ["a/file", "z"]);
    assert_eq!(rest.next_cursor, None);
    assert_eq!(
        snapshot.files(Some("a"), None, 10).unwrap().paths,
        ["a/file"]
    );
    assert_eq!(
        snapshot.files(Some("a.txt"), None, 10).unwrap().paths,
        ["a.txt"]
    );
    assert!(snapshot
        .files(Some("a/missing"), None, 10)
        .unwrap()
        .paths
        .is_empty());
    assert!(snapshot.read_file("a").unwrap().is_none());
    assert!(snapshot.read_file("a.txt/child").unwrap().is_none());
    assert!(snapshot.files(None, None, 0).is_err());
    assert!(snapshot.files(None, Some("../unsafe"), 10).is_err());
}

#[test]
fn cached_valid_bytes_do_not_bypass_live_fsck_or_new_session_validation() {
    let (dir, repo, revision) = fixture();
    let snapshot = repo.snapshot(revision).unwrap();
    let bytes = snapshot.read_file("z").unwrap().unwrap();
    let blob = repo.log(1).unwrap()[0]
        .1
        .changes
        .iter()
        .find(|c| c.path == "z")
        .unwrap()
        .blob
        .unwrap();
    let id = blob.to_string();
    fs::write(
        dir.path()
            .join(".loom/objects")
            .join(&id[..2])
            .join(&id[2..]),
        b"corrupt",
    )
    .unwrap();
    assert_eq!(&*snapshot.read_file("z").unwrap().unwrap(), &*bytes);
    assert!(repo.snapshot(revision).unwrap().read_file("z").is_err());
    assert!(repo.fsck().is_err());
    assert_eq!(repo.head().unwrap(), Some(revision));
}

#[test]
fn cache_budget_and_cache_disabled_mode_remain_correct() {
    let (_dir, repo, revision) = fixture();
    for budget in [0, 256, 1024] {
        let snapshot = repo.snapshot_with_cache_bytes(revision, budget).unwrap();
        for _ in 0..5 {
            assert_eq!(&*snapshot.read_file("a.txt").unwrap().unwrap(), b"first");
            assert_eq!(&*snapshot.read_file("z").unwrap().unwrap(), b"last");
            assert!(snapshot.stats().unwrap().cache_bytes <= budget);
        }
        if budget == 0 {
            assert_eq!(snapshot.stats().unwrap().cache_hits, 0);
        }
    }
    assert!(repo
        .snapshot_with_cache_bytes(revision, 256 * 1024 * 1024 + 1)
        .is_err());
}

#[test]
fn one_snapshot_can_serve_concurrent_readers_without_cross_revision_data() {
    let (_dir, repo, revision) = fixture();
    let snapshot = Arc::new(repo.snapshot(revision).unwrap());
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let snapshot = snapshot.clone();
            std::thread::spawn(move || {
                for _ in 0..100 {
                    assert_eq!(
                        &*snapshot.read_file("a/file").unwrap().unwrap(),
                        b"nested\0bytes\xff"
                    );
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    assert!(snapshot.stats().unwrap().cache_bytes <= loom_core::DEFAULT_SNAPSHOT_CACHE_BYTES);
}

#[test]
fn read_only_query_does_not_lock_recover_or_materialize_missing_indexes() {
    use fs2::FileExt;
    let (dir, repo, revision) = fixture();
    let db = rusqlite::Connection::open(dir.path().join(".loom/index.sqlite")).unwrap();
    db.execute(
        "INSERT INTO workspace_journal VALUES(1,'invalid-pending','main','main',NULL,?1)",
        [revision.to_string()],
    )
    .unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.path().join(".loom/workspace.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let path = dir.path().to_path_buf();
    let worker = std::thread::spawn(move || {
        let result = Repository::query_snapshot(path, revision, None, 1024)
            .and_then(|snapshot| snapshot.read_file("z"));
        send.send(result).unwrap();
    });
    let result = receive.recv_timeout(std::time::Duration::from_secs(5));
    FileExt::unlock(&lock).unwrap();
    worker.join().unwrap();
    assert_eq!(&*result.unwrap().unwrap().unwrap(), b"last");
    assert_eq!(
        db.query_row("SELECT count(*) FROM workspace_journal", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(fs::read(dir.path().join("z")).unwrap(), b"last");
    db.execute(
        "DELETE FROM revision_native_tree WHERE revision_oid=?1",
        [revision.to_string()],
    )
    .unwrap();
    let error = Repository::query_snapshot(dir.path(), revision, None, 0)
        .err()
        .unwrap();
    assert!(error.to_string().contains("loom tree"));
    assert_eq!(
        db.query_row("SELECT count(*) FROM revision_native_tree", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(repo.head().unwrap(), Some(revision));
}

#[test]
fn query_session_checks_requested_bytes_without_certifying_unrelated_history() {
    use loom_core::{sqlite_object_storage::SqliteObjectBackend, storage_backend::ObjectBackend};
    let workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let path = storage.path().join("objects.sqlite");
    let backend = Arc::new(SqliteObjectBackend::open(&path).unwrap());
    let repo = Repository::init_with_object_backend(workspace.path(), backend.clone()).unwrap();
    fs::write(workspace.path().join("file"), b"old").unwrap();
    repo.commit("test", "old").unwrap();
    let old_blob = repo.log(1).unwrap()[0].1.changes[0].blob.unwrap();
    fs::write(workspace.path().join("file"), b"current").unwrap();
    let tip = repo.commit("test", "current").unwrap();
    let current_blob = repo.log(1).unwrap()[0].1.changes[0].blob.unwrap();
    drop(repo);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "DELETE FROM loom_storage_objects WHERE oid=?1",
        [old_blob.to_string()],
    )
    .unwrap();
    let query =
        Repository::query_snapshot(workspace.path(), tip, Some(backend.clone()), 1024).unwrap();
    assert_eq!(&*query.read_file("file").unwrap().unwrap(), b"current");
    assert!(Repository::discover_with_object_backend(workspace.path(), backend.clone()).is_err());
    let uncached =
        Repository::query_snapshot(workspace.path(), tip, Some(backend.clone()), 0).unwrap();
    let files = uncached
        .read_files(&["file", "missing", "file"], 14)
        .unwrap();
    assert_eq!(&**files[0].as_ref().unwrap(), b"current");
    assert!(files[1].is_none());
    assert!(Arc::ptr_eq(
        files[0].as_ref().unwrap(),
        files[2].as_ref().unwrap()
    ));
    db.execute(
        "UPDATE loom_storage_objects SET envelope=x'00' WHERE oid=?1",
        [current_blob.to_string()],
    )
    .unwrap();
    assert_eq!(&*query.read_file("file").unwrap().unwrap(), b"current");
    let fresh =
        Repository::query_snapshot(workspace.path(), tip, Some(backend.clone()), 0).unwrap();
    assert!(fresh.read_file("file").is_err());
    assert!(backend.get(current_blob).is_err());
    assert!(Repository::discover_with_object_backend(workspace.path(), backend).is_err());
}

#[test]
fn bulk_reads_preserve_order_deduplicate_objects_and_count_repeated_bytes() {
    let (_dir, repo, revision) = fixture();
    let snapshot = repo.snapshot(revision).unwrap();
    let paths = ["z", "missing", "a.txt", "a", "a.txt"];
    let files = snapshot.read_files(&paths, 14).unwrap();
    assert_eq!(&**files[0].as_ref().unwrap(), b"last");
    assert!(files[1].is_none() && files[3].is_none());
    assert_eq!(&**files[2].as_ref().unwrap(), b"first");
    assert!(Arc::ptr_eq(
        files[2].as_ref().unwrap(),
        files[4].as_ref().unwrap()
    ));
    assert_eq!(snapshot.stats().unwrap().blob_reads, 2);
    assert!(snapshot.read_files(&paths, 13).is_err());
    assert!(snapshot.read_files(&["z", "../unsafe"], 100).is_err());
    assert!(snapshot.read_files(&vec!["z"; 4097], 100).is_err());
    assert!(snapshot
        .read_files(&["z"], loom_core::MAX_BULK_BYTES + 1)
        .is_err());
    assert!(snapshot.read_files::<&str>(&[], 0).unwrap().is_empty());
    assert!(snapshot.read_files(&["missing"], 0).unwrap()[0].is_none());
    assert!(repo.fsck().is_ok());
}

#[test]
fn bulk_reads_handle_many_directories_disabled_cache_and_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let mut paths = Vec::new();
    for n in 0..96 {
        let path = format!("dir{n}/file");
        fs::create_dir(dir.path().join(format!("dir{n}"))).unwrap();
        fs::write(dir.path().join(&path), format!("value {n}")).unwrap();
        paths.push(path);
    }
    let revision = repo.commit("test", "many directories").unwrap();
    let snapshot = repo.snapshot_with_cache_bytes(revision, 0).unwrap();
    for (n, bytes) in snapshot
        .read_files(&paths, 4096)
        .unwrap()
        .iter()
        .enumerate()
    {
        assert_eq!(&**bytes.as_ref().unwrap(), format!("value {n}").as_bytes());
    }
    assert_eq!(snapshot.stats().unwrap().blob_reads, 96);
    assert_eq!(snapshot.stats().unwrap().cache_bytes, 0);
    assert!(snapshot.read_files(&paths, 8).is_err());
    let blob = repo.log(1).unwrap()[0].1.changes[0]
        .blob
        .unwrap()
        .to_string();
    fs::write(
        dir.path()
            .join(".loom/objects")
            .join(&blob[..2])
            .join(&blob[2..]),
        b"corrupt",
    )
    .unwrap();
    assert!(snapshot.read_files(&paths, 4096).is_err());
    assert!(repo.fsck().is_err());
    assert_eq!(repo.head().unwrap(), Some(revision));
}
