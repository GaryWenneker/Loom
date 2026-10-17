use std::{fs, path::Path, process::Command};

fn call(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env("LOOM_AUTHOR", "sparse-export-test")
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

#[test]
fn sparse_export_is_revision_pinned_and_preserves_source_state() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    ok(&source, &["init"]);
    fs::create_dir_all(source.join("src/deep")).unwrap();
    fs::write(source.join("README.md"), b"committed readme").unwrap();
    fs::write(source.join("src/a.rs"), b"committed a").unwrap();
    fs::write(source.join("src/deep/b.rs"), b"committed b").unwrap();
    fs::write(source.join("outside.txt"), b"not selected").unwrap();
    let commit = String::from_utf8(ok(&source, &["commit", "-m", "snapshot"])).unwrap();
    let revision = commit.trim().strip_prefix("Committed ").unwrap();

    fs::write(source.join("README.md"), b"unstaged replacement").unwrap();
    fs::write(source.join("draft.txt"), b"staged draft").unwrap();
    ok(&source, &["add", "draft.txt"]);
    let status_before = ok(&source, &["status"]);
    let destination = temp.path().join("agent-one");
    let destination_text = destination.to_str().unwrap();
    let output = ok(
        &source,
        &[
            "sparse-export",
            &format!("@{revision}"),
            destination_text,
            "src",
            "README.md",
        ],
    );

    assert!(String::from_utf8(output).unwrap().contains("Exported 3 files"));
    assert_eq!(
        fs::read(destination.join("README.md")).unwrap(),
        b"committed readme"
    );
    assert_eq!(fs::read(destination.join("src/a.rs")).unwrap(), b"committed a");
    assert_eq!(
        fs::read(destination.join("src/deep/b.rs")).unwrap(),
        b"committed b"
    );
    assert!(!destination.join("outside.txt").exists());
    assert!(!destination.join("draft.txt").exists());
    assert!(destination.join(".loom-sparse-complete").is_file());
    assert_eq!(ok(&source, &["status"]), status_before);
    assert_eq!(fs::read(source.join("README.md")).unwrap(), b"unstaged replacement");
    assert_eq!(fs::read(source.join("draft.txt")).unwrap(), b"staged draft");

    let failed = call(
        &source,
        &[
            "sparse-export",
            &format!("@{revision}"),
            destination_text,
            "src",
        ],
    );
    assert!(!failed.status.success());
    assert_eq!(
        fs::read(destination.join("README.md")).unwrap(),
        b"committed readme"
    );
}
