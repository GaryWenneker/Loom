use std::{fs, path::Path, process::Command};

fn output(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(dir)
        .env("LOOM_AUTHOR", "list-test")
        .args(args)
        .output()
        .expect("launch loom")
}

fn ok(dir: &Path, args: &[&str]) -> Vec<u8> {
    let result = output(dir, args);
    assert!(
        result.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

#[test]
fn files_query_lists_indexed_and_unindexed_branch_trees_without_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::create_dir_all(root.join("src/deep")).unwrap();
    fs::create_dir_all(root.join("src2")).unwrap();
    fs::write(root.join("src/entry.rs"), b"base").unwrap();
    fs::write(root.join("src/deep/nested.rs"), b"nested").unwrap();
    fs::write(root.join("src2/other.rs"), b"not the src prefix").unwrap();
    fs::write(root.join("read me.txt"), b"spaces").unwrap();
    ok(root, &["commit", "-m", "legacy base"]);

    // Legacy unindexed history must give the same sorted results.
    let before = String::from_utf8(ok(root, &["files", "main", "src"])).unwrap();
    assert_eq!(before, "\"src/deep/nested.rs\"\n\"src/entry.rs\"\n");
    assert_eq!(ok(root, &["files", "main", "missing"]), b"");

    fs::write(root.join("src/entry.rs"), b"indexed").unwrap();
    ok(root, &["add", "src/entry.rs"]);
    ok(root, &["commit", "-m", "indexed commit"]);
    let indexed = ok(root, &["files", "main", "src"]);
    assert_eq!(indexed, before.as_bytes());
    assert_eq!(ok(root, &["cat", "main", "src/entry.rs"]), b"indexed");

    let all = ok(root, &["files", "main", "--null"]);
    assert_eq!(
        all,
        b"read me.txt\0src/deep/nested.rs\0src/entry.rs\0src2/other.rs\0"
    );
    assert!(!String::from_utf8(ok(root, &["files", "main", "src"])).unwrap()
        .contains("src2"));
    ok(root, &["fsck"]);
}

#[test]
fn indexed_file_list_handles_deletion_and_branch_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join("shared"), b"shared").unwrap();
    ok(root, &["commit", "-m", "base"]);
    ok(root, &["branch", "create", "feature"]);
    ok(root, &["switch", "feature"]);

    fs::write(root.join("feature-only.txt"), b"feature").unwrap();
    ok(root, &["add", "feature-only.txt"]);
    ok(root, &["commit", "-m", "feature"]);
    assert_eq!(ok(root, &["files", "feature", "feature-only.txt"]),
               b"\"feature-only.txt\"\n");

    ok(root, &["switch", "main"]);
    assert_eq!(ok(root, &["files", "main", "feature-only.txt"]), b"");
    assert_eq!(ok(root, &["files", "feature", "feature-only.txt"]),
               b"\"feature-only.txt\"\n");

    fs::remove_file(root.join("shared")).unwrap();
    ok(root, &["add", "shared"]);
    ok(root, &["commit", "-m", "remove on main"]);
    assert_eq!(ok(root, &["files", "main", "shared"]), b"");
    assert_eq!(ok(root, &["files", "feature", "shared"]), b"\"shared\"\n");
    assert!(!output(root, &["files", "unknown"]).status.success());
    ok(root, &["fsck"]);
}

#[cfg(unix)]
#[test]
fn nul_output_preserves_embedded_newlines_without_ambiguous_records() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join("multi\nline.txt"), b"file").unwrap();
    ok(root, &["add", "multi\nline.txt"]);
    ok(root, &["commit", "-m", "newline filename"]);
    let quoted = String::from_utf8(ok(root, &["files", "main"])).unwrap();
    assert_eq!(quoted, "\"multi\\nline.txt\"\n");
    assert_eq!(ok(root, &["files", "main", "--null"]), b"multi\nline.txt\0");
}
