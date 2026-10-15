use std::{fs, path::Path, process::Command};

fn call(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env("LOOM_AUTHOR", "historical-tree-test")
        .args(args).output().unwrap()
}

fn ok(root: &Path, args: &[&str]) -> Vec<u8> {
    let result = call(root, args);
    assert!(result.status.success(),
        "{args:?}: {}", String::from_utf8_lossy(&result.stderr));
    result.stdout
}

fn revision(root: &Path, message: &str) -> String {
    let output = ok(root, &["commit", "-m", message]);
    let id = std::str::from_utf8(&output).unwrap()
        .trim().strip_prefix("Committed ").unwrap().to_owned();
    assert_eq!(id.len(), 64);
    id
}

#[test]
fn list_exact_historical_trees_without_modifying_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/old.rs"), b"old").unwrap();
    fs::write(root.join("README.md"), b"readme").unwrap();
    let base = revision(root, "base");

    fs::remove_file(root.join("src/old.rs")).unwrap();
    fs::write(root.join("src/new.rs"), b"new").unwrap();
    let updated = revision(root, "updated");
    let old_ref = format!("@{base}");
    let new_ref = format!("@{updated}");

    // An immutable revision remains queryable even when its files have been
    // removed from the current worktree.
    for _ in 0..2 {
        assert_eq!(ok(root, &["files", &old_ref, "src"]),
                   b"\"src/old.rs\"\n");
        assert_eq!(ok(root, &["files", &new_ref, "src"]),
                   b"\"src/new.rs\"\n");
        assert_eq!(ok(root, &["files", &old_ref, "--null"]),
                   b"README.md\0src/old.rs\0");
    }
    assert_eq!(ok(root, &["files", "main", "src"]), b"\"src/new.rs\"\n");
    assert_eq!(fs::read(root.join("src/new.rs")).unwrap(), b"new");
    assert!(!root.join("src/old.rs").exists());
    assert!(!call(root, &["files", "@garbage"]).status.success());
    ok(root, &["fsck"]);
}

#[cfg(unix)]
#[test]
fn historical_tree_null_output_handles_embedded_newlines() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join("a\nmultiline"), b"contents").unwrap();
    let id = revision(root, "newline");
    let reference = format!("@{id}");
    assert_eq!(ok(root, &["files", &reference, "--null"]), b"a\nmultiline\0");
    assert_eq!(ok(root, &["files", &reference]), b"\"a\\nmultiline\"\n");
}
