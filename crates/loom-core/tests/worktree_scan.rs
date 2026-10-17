use loom_core::{ChangeKind, Repository, StatusKind, WorktreeScanOptions};
use std::fs;

#[test]
fn serial_and_parallel_commits_preserve_roots_and_all_change_kinds() {
    let serial = tempfile::tempdir().unwrap();
    let parallel = tempfile::tempdir().unwrap();
    let mut roots = Vec::new();
    let mut histories = Vec::new();
    for (directory, workers) in [(&serial, 1), (&parallel, usize::MAX)] {
        let repo = Repository::init(directory.path()).unwrap();
        for n in 0..2200 {
            let path = directory
                .path()
                .join(format!("dir/{:03}/{n:04}.bin", n % 64));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, [n as u8, 0, 255, 10]).unwrap();
        }
        // Exercise byte-bounded windows, the isolated large-file path, and
        // repeated content IDs across independent scan/publication windows.
        fs::write(directory.path().join("large"), vec![42; 9 * 1024 * 1024]).unwrap();
        let first = repo
            .commit_with_scan_options("test", "base", WorktreeScanOptions { workers })
            .unwrap();
        assert!(repo.status().unwrap().is_empty());
        for n in 0..40 {
            let path = directory
                .path()
                .join(format!("dir/{:03}/{n:04}.bin", n % 64));
            if n % 2 == 0 {
                fs::remove_file(path).unwrap();
            } else {
                fs::write(path, b"changed\0\xff").unwrap();
            }
        }
        fs::write(directory.path().join("new"), []).unwrap();
        let status = repo
            .status_with_scan_options(WorktreeScanOptions { workers })
            .unwrap();
        assert_eq!(
            status
                .iter()
                .filter(|x| x.kind == StatusKind::Deleted)
                .count(),
            20
        );
        assert_eq!(
            status
                .iter()
                .filter(|x| x.kind == StatusKind::Modified)
                .count(),
            20
        );
        assert_eq!(
            status
                .iter()
                .filter(|x| x.kind == StatusKind::Added)
                .count(),
            1
        );
        let second = repo
            .commit_with_scan_options("test", "change", WorktreeScanOptions { workers })
            .unwrap();
        let history = repo.log(1).unwrap().remove(0).1.changes;
        assert_eq!(
            history
                .iter()
                .filter(|x| x.kind == ChangeKind::Delete)
                .count(),
            20
        );
        roots.push((
            repo.native_tree_root(first).unwrap(),
            repo.native_tree_root(second).unwrap(),
        ));
        histories.push(history);
        assert_eq!(
            repo.read_revision_file(second, "new").unwrap(),
            Some(vec![])
        );
        assert_eq!(
            repo.read_revision_file(second, "dir/001/0001.bin").unwrap(),
            Some(b"changed\0\xff".to_vec())
        );
        assert!(repo.status().unwrap().is_empty());
        repo.fsck().unwrap();
    }
    assert_eq!(roots[0], roots[1]);
    assert_eq!(histories[0], histories[1]);
}

#[test]
fn ignore_rules_never_hide_tracked_changes_or_staged_post_edits() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Repository::init(temp.path()).unwrap();
    fs::write(temp.path().join("tracked.tmp"), b"base").unwrap();
    repo.commit("test", "base").unwrap();
    fs::write(temp.path().join(".loomignore"), "*.tmp\n").unwrap();
    fs::write(temp.path().join("untracked.tmp"), b"ignored").unwrap();
    fs::write(temp.path().join("tracked.tmp"), b"staged").unwrap();
    let changes = repo.status().unwrap();
    assert_eq!(changes.len(), 2);
    repo.add("tracked.tmp").unwrap();
    let before = repo.unstaged_status().unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].path, ".loomignore");
    fs::write(temp.path().join("tracked.tmp"), b"after staging").unwrap();
    assert_eq!(repo.unstaged_status().unwrap().len(), 2);
    let tip = repo
        .commit_with_scan_options(
            "test",
            "only staged",
            WorktreeScanOptions {
                workers: usize::MAX,
            },
        )
        .unwrap();
    assert_eq!(
        repo.read_revision_file(tip, "tracked.tmp").unwrap(),
        Some(b"staged".to_vec())
    );
    assert_eq!(repo.unstaged_status().unwrap().len(), 2);
}

#[test]
fn oversized_input_does_not_publish_a_ref_and_retry_is_lossless() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Repository::init(temp.path()).unwrap();
    fs::write(temp.path().join("existing"), b"base").unwrap();
    let base = repo.commit("test", "base").unwrap();
    let file = fs::File::create(temp.path().join("oversized")).unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    assert!(repo.commit("test", "reject").is_err());
    assert_eq!(repo.head().unwrap(), Some(base));
    fs::remove_file(temp.path().join("oversized")).unwrap();
    fs::write(temp.path().join("existing"), b"retry").unwrap();
    let tip = repo.commit("test", "retry").unwrap();
    assert_eq!(
        repo.read_revision_file(tip, "existing").unwrap(),
        Some(b"retry".to_vec())
    );
    repo.fsck().unwrap();
}
