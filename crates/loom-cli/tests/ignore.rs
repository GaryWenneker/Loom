use std::{fs, path::Path, process::Command};

fn cmd(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env("LOOM_AUTHOR", "ignore-test")
        .args(args)
        .output()
        .unwrap()
}

fn ok(root: &Path, args: &[&str]) -> String {
    let output = cmd(root, args);
    assert!(output.status.success(), "{:?}: {}", args, String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn ignores_untracked_globs_but_commits_loomignore_itself() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join(".loomignore"), "*.log\n/cache/\n").unwrap();
    fs::write(root.join("debug.log"), b"ignored").unwrap();
    fs::create_dir(root.join("cache")).unwrap();
    fs::write(root.join("cache").join("data.bin"), b"ignored").unwrap();
    fs::write(root.join("keep.txt"), b"tracked").unwrap();
    let status = ok(root, &["status"]);
    assert!(status.contains("A .loomignore"), "{status}");
    assert!(status.contains("A keep.txt"), "{status}");
    assert!(!status.contains("debug.log"), "{status}");
    assert!(!status.contains("data.bin"), "{status}");
    ok(root, &["commit", "-m", "root ignore rules"]);
    assert!(ok(root, &["status"]).contains("Working tree clean"));
    assert!(!cmd(root, &["cat", "main", "debug.log"]).status.success());
    assert!(!cmd(root, &["cat", "main", "cache/data.bin"]).status.success());
    assert!(ok(root, &["cat", "main", ".loomignore"]).contains("*.log"));
    assert!(cmd(root, &["add", "debug.log"]).status.success() == false);
    ok(root, &["fsck"]);
}

#[test]
fn tracked_file_remains_visible_after_new_ignore_pattern() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join("tracked.log"), "v1").unwrap();
    ok(root, &["commit", "-m", "track log"]);
    fs::write(root.join(".loomignore"), "*.log\n").unwrap();
    fs::write(root.join("tracked.log"), "v2").unwrap();
    fs::write(root.join("untracked.log"), "ignore").unwrap();
    let status = ok(root, &["status"]);
    assert!(status.contains("M tracked.log"), "{status}");
    assert!(!status.contains("untracked.log"), "{status}");
    ok(root, &["add", "tracked.log", ".loomignore"]);
    ok(root, &["commit", "-m", "update tracked despite ignore"]);
    assert_eq!(ok(root, &["cat", "main", "tracked.log"]), "v2");
    assert!(!cmd(root, &["cat", "main", "untracked.log"]).status.success());
}

#[test]
fn ignored_oversized_file_does_not_block_status_or_initial_commit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join(".loomignore"), "*.cache\n").unwrap();
    let file = fs::File::create(root.join("huge.cache")).unwrap();
    file.set_len(65 * 1024 * 1024).unwrap();
    let status = ok(root, &["status"]);
    assert!(!status.contains("huge.cache"), "{status}");
    ok(root, &["commit", "-m", "small files only"]);
    assert!(!cmd(root, &["cat", "main", "huge.cache"]).status.success());
}

#[test]
fn invalid_ignore_rules_fail_without_silently_tracking_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join(".loomignore"), "!foo\n").unwrap();
    fs::write(root.join("foo"), "must not silently commit").unwrap();
    let out = cmd(root, &["commit", "-m", "fail closed"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains(".loomignore"));
}
