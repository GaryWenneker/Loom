use std::{fs, path::Path, process::Command};

fn call(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env("LOOM_AUTHOR", "fast-forward-test")
        .args(args).output().unwrap()
}
fn ok(root: &Path, args: &[&str]) -> String {
    let out = call(root, args);
    assert!(out.status.success(),
            "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}
fn commit_id(text: &str) -> String {
    text.trim().strip_prefix("Committed ").unwrap().to_owned()
}

#[test]
fn fast_forward_multiple_changesets_updates_branch_without_merge_commit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    fs::write(root.join("root"), b"root").unwrap();
    ok(root, &["commit", "-m", "root"]);
    ok(root, &["branch", "create", "feature"]);
    ok(root, &["switch", "feature"]);
    fs::write(root.join("new"), b"v1").unwrap();
    ok(root, &["commit", "-m", "first"]);
    fs::write(root.join("new"), b"v2").unwrap();
    let target = commit_id(&ok(root, &["commit", "-m", "second"]));
    ok(root, &["switch", "main"]);
    assert!(!root.join("new").exists());
    let result = ok(root, &["merge", "feature"]);
    assert!(result.starts_with("Fast-forwarded to feature ("), "{result}");
    assert!(result.contains(&target));
    assert_eq!(fs::read(root.join("new")).unwrap(), b"v2");
    let log = ok(root, &["log", "1"]);
    assert!(log.contains(&target), "{log}");
    let repeated = ok(root, &["merge", "feature"]);
    assert!(repeated.contains("Already up to date"), "{repeated}");
    ok(root, &["fsck"]);
}

#[test]
fn merging_an_old_branch_does_not_create_redundant_history() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    fs::write(root.join("a"), b"base").unwrap();
    ok(root, &["commit", "-m", "base"]);
    ok(root, &["branch", "create", "old"]);
    fs::write(root.join("a"), b"new").unwrap();
    let latest = commit_id(&ok(root, &["commit", "-m", "new"]));
    let out = ok(root, &["merge", "old"]);
    assert!(out.starts_with("Already up to date with old"), "{out}");
    assert!(out.contains(&latest));
    assert_eq!(fs::read(root.join("a")).unwrap(), b"new");
    ok(root, &["fsck"]);
}
