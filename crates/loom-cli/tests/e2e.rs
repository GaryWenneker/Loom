use std::{fs, path::Path, process::Command};

fn run(dir: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(dir)
        .env("LOOM_AUTHOR", "integration-test")
        .args(args)
        .output()
        .expect("run loom");
    assert!(
        output.status.success(),
        "loom {:?} failed:\nstdout: {}\nstderr: {}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn init_commit_branch_switch_merge_verify() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    run(root, &["init"]);
    assert!(root.join(".loom").exists());
    fs::write(root.join("a.txt"), "hello").unwrap();
    assert!(run(root, &["status"]).contains("A a.txt"));
    run(root, &["commit", "-m", "first"]);
    assert!(run(root, &["status"]).contains("Working tree clean"));
    run(root, &["branch", "create", "feature"]);
    run(root, &["switch", "feature"]);
    fs::write(root.join("b.txt"), "feature").unwrap();
    run(root, &["commit", "-m", "feature commit"]);
    run(root, &["switch", "main"]);
    assert!(!root.join("b.txt").exists());
    fs::write(root.join("a.txt"), "main edit").unwrap();
    run(root, &["commit", "-m", "main edit"]);
    run(root, &["merge", "feature"]);
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "main edit");
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "feature");
    assert!(run(root, &["fsck"]).contains("Verified"));
    assert!(run(root, &["log"]).contains("Merge branch feature"));
}

#[test]
fn invalid_commands_fail() {
    let dir = tempfile::tempdir().unwrap();
    run(dir.path(), &["init"]);
    let output = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(dir.path())
        .args(["commit", "-m", "nothing"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("nothing to commit"));
}
