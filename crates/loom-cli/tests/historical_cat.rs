use std::{fs, path::Path, process::Command};

fn call(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env("LOOM_AUTHOR", "history-test")
        .args(args)
        .output()
        .expect("execute loom")
}

fn ok(root: &Path, args: &[&str]) -> Vec<u8> {
    let result = call(root, args);
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

fn commit_id(output: &[u8]) -> String {
    let message = std::str::from_utf8(output).unwrap();
    let id = message.trim().strip_prefix("Committed ").unwrap();
    assert_eq!(id.len(), 64);
    id.to_owned()
}

#[test]
fn cat_reads_any_native_revision_without_switching_branches() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    fs::write(root.join("file.txt"), b"first").unwrap();
    let initial = commit_id(&ok(root, &["commit", "-m", "initial"]));
    fs::write(root.join("file.txt"), b"second").unwrap();
    let updated = commit_id(&ok(root, &["commit", "-m", "updated"]));
    let first_ref = format!("@{initial}");
    let second_ref = format!("@{updated}");

    for _ in 0..2 {
        assert_eq!(ok(root, &["cat", &first_ref, "file.txt"]), b"first");
        assert_eq!(ok(root, &["cat", &second_ref, "file.txt"]), b"second");
        assert_eq!(ok(root, &["cat", "main", "file.txt"]), b"second");
    }
    assert!(!call(root, &["cat", &first_ref, "absent"]).status.success());
    assert!(!call(root, &["cat", "@xyz", "file.txt"]).status.success());
    assert_eq!(fs::read(root.join("file.txt")).unwrap(), b"second");
    ok(root, &["fsck"]);
}

#[test]
fn historical_binary_data_is_written_without_text_encoding() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    let bytes = [0u8, 255u8, 8u8, 0u8, 254u8];
    fs::write(root.join("data.bin"), bytes).unwrap();
    let id = commit_id(&ok(root, &["commit", "-m", "binary"]));
    assert_eq!(ok(root, &["cat", &format!("@{id}"), "data.bin"]), bytes);
}
