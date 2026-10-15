use std::{fs, path::Path, process::Command};

fn cmd(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env("LOOM_AUTHOR", "staging-test")
        .args(args)
        .output()
        .expect("execute loom")
}

fn ok(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = cmd(root, args);
    assert!(
        output.status.success(),
        "loom {:?} failed: {}",
        args, String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn staged_snapshot_is_immutable_and_only_selected_paths_are_committed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join("a.txt"), b"first\n").unwrap();
    fs::write(root.join("b.txt"), b"not selected\n").unwrap();

    ok(root, &["add", "a.txt"]);
    // Capture at 'add' time, rather than accidentally committing newer working bytes.
    fs::write(root.join("a.txt"), b"edited after staging\n").unwrap();
    let status = String::from_utf8(ok(root, &["status"])).unwrap();
    assert!(status.contains("staged A a.txt"), "{status}");
    assert!(status.contains("A b.txt"), "{status}");

    ok(root, &["commit", "-m", "only a"]);
    assert_eq!(ok(root, &["cat", "main", "a.txt"]), b"first\n");
    assert!(!cmd(root, &["cat", "main", "b.txt"]).status.success());

    assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"edited after staging\n");
    assert_eq!(fs::read(root.join("b.txt")).unwrap(), b"not selected\n");
    assert!(!cmd(root, &["commit", "-m", "should fail"]).status.success());
    assert!(String::from_utf8(ok(root, &["status"])).unwrap().contains("No changes staged"));

    ok(root, &["add", "a.txt", "b.txt"]);
    ok(root, &["commit", "-m", "both"]);
    assert_eq!(ok(root, &["cat", "main", "a.txt"]), b"edited after staging\n");
    assert_eq!(ok(root, &["cat", "main", "b.txt"]), b"not selected\n");
    assert!(String::from_utf8(ok(root, &["status"])).unwrap().contains("Working tree clean"));
    ok(root, &["fsck"]);
}

#[test]
fn stage_tracked_deletion_and_unstage_without_losing_working_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join("keep"), b"keep").unwrap();
    fs::write(root.join("delete"), b"delete").unwrap();
    ok(root, &["commit", "-m", "initial legacy commit"]);
    fs::remove_file(root.join("delete")).unwrap();
    ok(root, &["add", "delete"]);
    assert!(String::from_utf8(ok(root, &["status"])).unwrap().contains("staged D delete"));
    ok(root, &["unstage", "delete"]);
    assert!(!cmd(root, &["commit", "-m", "nothing staged"]).status.success());

    ok(root, &["add", "delete"]);
    ok(root, &["commit", "-m", "removed"]);
    assert!(!cmd(root, &["cat", "main", "delete"]).status.success());
    assert_eq!(ok(root, &["cat", "main", "keep"]), b"keep");
    ok(root, &["fsck"]);
}

#[test]
fn cannot_switch_branches_with_staged_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join("tracked"), b"one").unwrap();
    ok(root, &["commit", "-m", "base"]);
    ok(root, &["branch", "create", "feature"]);
    fs::write(root.join("tracked"), b"two").unwrap();
    ok(root, &["add", "tracked"]);
    let rejected = cmd(root, &["switch", "feature"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("staged"));
    ok(root, &["unstage", "tracked"]);
    // Now unstaged worktree edits remain a separate, correct blocker.
    assert!(!cmd(root, &["switch", "feature"]).status.success());
    assert_eq!(fs::read(root.join("tracked")).unwrap(), b"two");
}
