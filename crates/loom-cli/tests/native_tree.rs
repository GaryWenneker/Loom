use std::{fs, path::Path, process::Command};

fn call(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env("LOOM_AUTHOR", "native-tree-test")
        .args(args)
        .output()
        .expect("execute loom")
}

fn ok(root: &Path, args: &[&str]) -> String {
    let out = call(root, args);
    assert!(out.status.success(),
        "{args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn immutable_tree_roots_are_repeatable_and_do_not_change_v1_revisions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::create_dir_all(root.join("src/deep")).unwrap();
    fs::write(root.join("src/deep/lib.rs"), b"pub fn f() {}").unwrap();
    fs::write(root.join("README.md"), b"README").unwrap();

    let commit = ok(root, &["commit", "-m", "legacy v1 changeset"]);
    let id = commit.trim().strip_prefix("Committed ").unwrap();
    assert_eq!(id.len(), 64);
    let reference = format!("@{id}");
    let first = ok(root, &["tree", &reference]);
    assert_eq!(first.trim().len(), 64);
    let second = ok(root, &["tree", &reference]);
    assert_eq!(first, second, "canonical object ID must be stable");
    assert_eq!(ok(root, &["files", &reference, "src"]),
               "\"src/deep/lib.rs\"\n");
    assert_eq!(fs::read(root.join("README.md")).unwrap(), b"README");

    ok(root, &["fsck"]);
    assert!(!call(root, &["tree", "main"]).status.success());
    assert!(!call(root, &["tree", "@invalid"]).status.success());

    fs::write(root.join("README.md"), b"changed").unwrap();
    let next_commit = ok(root, &["commit", "-m", "updated readme"]);
    let next_id = next_commit.trim().strip_prefix("Committed ").unwrap();
    assert_ne!(first, ok(root, &["tree", &format!("@{next_id}")]));
    assert_eq!(first, ok(root, &["tree", &reference]));
    ok(root, &["fsck"]);
}

#[test]
fn empty_native_trees_are_stable_and_validate() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join("transient"), b"x").unwrap();
    ok(root, &["commit", "-m", "first"]);
    fs::remove_file(root.join("transient")).unwrap();
    let out = ok(root, &["commit", "-m", "deleted"]);
    let id = out.trim().strip_prefix("Committed ").unwrap();
    let first = ok(root, &["tree", &format!("@{id}")]);
    assert_eq!(first.trim().len(), 64);
    assert!(ok(root, &["files", &format!("@{id}")]).is_empty());
    assert_eq!(first, ok(root, &["tree", &format!("@{id}")]));
    ok(root, &["fsck"]);
}
