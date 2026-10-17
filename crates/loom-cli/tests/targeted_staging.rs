use std::{fs, path::Path, process::Command};

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env("LOOM_AUTHOR", "targeted-test")
        .args(args)
        .output()
        .unwrap()
}

fn ok(root: &Path, args: &[&str]) -> Vec<u8> {
    let result = run(root, args);
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

#[test]
fn staged_commit_does_not_read_unrelated_large_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join("selected.txt"), b"selected version").unwrap();
    let huge = fs::File::create(root.join("unrelated.big")).unwrap();
    huge.set_len(65 * 1024 * 1024).unwrap();

    // Before this change, both 'add' and 'commit' scanned every file,
    // so an unrelated 65 MiB file blocked a small staged commit.
    ok(root, &["add", "selected.txt"]);
    ok(root, &["commit", "-m", "single file"]);
    assert_eq!(
        ok(root, &["cat", "main", "selected.txt"]),
        b"selected version"
    );
    ok(root, &["fsck"]);
    assert_eq!(
        fs::metadata(root.join("unrelated.big")).unwrap().len(),
        65 * 1024 * 1024
    );
}

#[test]
fn staged_deletion_does_not_need_unrelated_worktree_access() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join("tracked.txt"), b"initial").unwrap();
    ok(root, &["commit", "-m", "base"]);
    fs::remove_file(root.join("tracked.txt")).unwrap();
    let huge = fs::File::create(root.join("unrelated.big")).unwrap();
    huge.set_len(65 * 1024 * 1024).unwrap();
    ok(root, &["add", "tracked.txt"]);
    ok(root, &["commit", "-m", "remove tracked"]);
    assert!(!run(root, &["cat", "main", "tracked.txt"]).status.success());
    ok(root, &["fsck"]);
}

#[test]
fn multidir_add_and_unstage_are_atomic_cli_operations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    for path in ["src/a", "docs/a", "other/a"] {
        fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        fs::write(root.join(path), b"first").unwrap();
    }
    assert!(!run(root, &["add", "src", "missing"]).status.success());
    assert!(run(root, &["diff", "--staged"]).stdout.is_empty());
    ok(root, &["add", "src", "docs"]);
    ok(root, &["unstage", "docs"]);
    ok(root, &["commit", "-m", "source only"]);
    assert_eq!(ok(root, &["cat", "main", "src/a"]), b"first");
    assert!(!run(root, &["cat", "main", "docs/a"]).status.success());
    assert!(!run(root, &["cat", "main", "other/a"]).status.success());
    fs::remove_dir_all(root.join("src")).unwrap();
    ok(root, &["add", "src"]);
    ok(root, &["commit", "-m", "remove subtree"]);
    assert!(!run(root, &["cat", "main", "src/a"]).status.success());
    ok(root, &["fsck"]);
}

#[test]
fn add_worker_override_is_validated_and_keeps_the_pathset_atomic() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join("a"), b"first").unwrap();
    fs::write(root.join("b"), b"second").unwrap();
    fs::write(root.join("kept"), b"already prepared").unwrap();
    ok(root, &["add", "kept"]);
    let staged_before = ok(root, &["diff", "--staged"]);

    for args in [
        vec![
            "add",
            "--scan-workers",
            "1",
            "a",
            "--scan-workers",
            "2",
            "b",
        ],
        vec!["add", "a", "--scan-workers", "invalid"],
        vec!["add", "a", "--scan-workers", "-1"],
        vec!["add", "a", "--scan-workers", "18446744073709551616"],
        vec!["add", "a", "--scan-workers"],
        vec!["add", "--scan-workers", "1"],
        vec!["add", "--"],
    ] {
        let result = run(root, &args);
        assert!(!result.status.success(), "{args:?}");
        assert!(
            result.stdout.is_empty(),
            "failed add printed success: {args:?}"
        );
        assert_eq!(ok(root, &["diff", "--staged"]), staged_before);
    }

    ok(root, &["add", "--scan-workers", "1", "a"]);
    ok(root, &["add", "--scan-workers", "0", "b"]);
    ok(root, &["add", "--scan-workers", "1024", "a", "b"]);
    ok(root, &["commit", "-m", "bounded workers"]);
    assert_eq!(ok(root, &["cat", "main", "a"]), b"first");
    assert_eq!(ok(root, &["cat", "main", "b"]), b"second");
    assert_eq!(ok(root, &["cat", "main", "kept"]), b"already prepared");
    ok(root, &["fsck"]);
}

#[test]
fn add_option_terminator_preserves_literal_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join("--scan-workers"), b"literal option name").unwrap();
    fs::write(root.join("--"), b"literal terminator").unwrap();
    fs::write(root.join("unselected"), b"leave unselected").unwrap();
    ok(
        root,
        &["add", "--scan-workers", "8", "--", "--scan-workers", "--"],
    );
    ok(root, &["commit", "-m", "literal selected paths"]);
    assert_eq!(
        ok(root, &["cat", "main", "--scan-workers"]),
        b"literal option name"
    );
    assert_eq!(ok(root, &["cat", "main", "--"]), b"literal terminator");
    assert!(!run(root, &["cat", "main", "unselected"]).status.success());
    ok(root, &["fsck"]);
}

#[cfg(unix)]
#[test]
fn staging_rejects_symlinked_parent_without_leaking_external_content() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.txt"), b"never stage").unwrap();
    symlink(outside.path(), root.join("linked")).unwrap();
    let result = run(root, &["add", "linked/secret.txt"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("symlink"));
    assert_eq!(
        fs::read(outside.path().join("secret.txt")).unwrap(),
        b"never stage"
    );
}
