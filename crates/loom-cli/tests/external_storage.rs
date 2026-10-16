use std::{fs, path::Path, process::Command};

fn run(root: &Path, objects: Option<&Path>, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_loom"));
    command
        .current_dir(root)
        .env("LOOM_AUTHOR", "external-test")
        .env_remove("LOOM_OBJECT_DIR")
        .args(args);
    if let Some(objects) = objects {
        command.env("LOOM_OBJECT_DIR", objects);
    }
    command.output().unwrap()
}
fn ok(root: &Path, objects: Option<&Path>, args: &[&str]) -> Vec<u8> {
    let output = run(root, objects, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn cli_external_object_directory_roundtrips_and_requires_explicit_binding() {
    let workspace = tempfile::tempdir().unwrap();
    let objects = tempfile::tempdir().unwrap();
    let dir = objects.path().join("objects");
    ok(
        workspace.path(),
        None,
        &["init", "--object-dir", dir.to_str().unwrap()],
    );
    fs::write(workspace.path().join("file"), b"external content").unwrap();
    ok(workspace.path(), Some(&dir), &["add", "file"]);
    ok(workspace.path(), Some(&dir), &["commit", "-m", "external"]);
    assert_eq!(
        ok(workspace.path(), Some(&dir), &["cat", "main", "file"]),
        b"external content"
    );
    ok(workspace.path(), Some(&dir), &["fsck"]);
    assert_eq!(
        fs::read_dir(workspace.path().join(".loom/objects"))
            .unwrap()
            .count(),
        0
    );
    let rejected = run(workspace.path(), None, &["status"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("LOOM_OBJECT_DIR"));
    assert_eq!(
        fs::read(workspace.path().join("file")).unwrap(),
        b"external content"
    );
}

#[test]
fn malformed_init_arguments_do_not_create_repository_metadata() {
    for args in [
        vec!["init", "--object-dir"],
        vec!["init", "--unknown"],
        vec!["init", ".", "unexpected"],
    ] {
        let workspace = tempfile::tempdir().unwrap();
        assert!(!run(workspace.path(), None, &args).status.success());
        assert!(!workspace.path().join(".loom").exists());
    }
}
