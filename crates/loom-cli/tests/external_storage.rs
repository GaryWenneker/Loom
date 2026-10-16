use std::{fs, path::Path, process::Command};

fn run(root: &Path, objects: Option<&Path>, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_loom"));
    command
        .current_dir(root)
        .env("LOOM_AUTHOR", "external-test")
        .env_remove("LOOM_OBJECT_DIR")
        .env_remove("LOOM_OBJECT_DB")
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

#[test]
fn sqlite_objects_work_through_cli_across_processes() {
    let workspace = tempfile::tempdir().unwrap();
    let objects = tempfile::tempdir().unwrap();
    let db = objects.path().join("objects.sqlite");
    ok(
        workspace.path(),
        None,
        &["init", "--object-db", db.to_str().unwrap()],
    );
    fs::write(workspace.path().join("file"), b"SQLite bytes").unwrap();
    let run_db = |args: &[&str]| {
        let result = Command::new(env!("CARGO_BIN_EXE_loom"))
            .current_dir(workspace.path())
            .env_remove("LOOM_OBJECT_DIR")
            .env("LOOM_OBJECT_DB", &db)
            .env("LOOM_AUTHOR", "sqlite-test")
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        result.stdout
    };
    run_db(&["add", "file"]);
    run_db(&["commit", "-m", "SQLite object provider"]);
    assert_eq!(run_db(&["cat", "main", "file"]), b"SQLite bytes");
    run_db(&["branch", "create", "feature"]);
    run_db(&["switch", "feature"]);
    run_db(&["fsck"]);
    assert_eq!(
        fs::read_dir(workspace.path().join(".loom/objects"))
            .unwrap()
            .count(),
        0
    );
    assert!(!run(workspace.path(), None, &["status"]).status.success());
    let rejected = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(workspace.path())
        .env("LOOM_OBJECT_DIR", objects.path())
        .env("LOOM_OBJECT_DB", &db)
        .arg("status")
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("choose one object provider"));
}

#[test]
fn provider_files_cannot_be_tracked_inside_their_own_worktree() {
    for (option, name) in [
        ("--object-dir", "objects"),
        ("--object-db", "objects.sqlite"),
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let target = workspace.path().join(name);
        let result = run(
            workspace.path(),
            None,
            &["init", option, target.to_str().unwrap()],
        );
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("outside the worktree"));
        assert!(!workspace.path().join(".loom").exists());
        assert!(!target.exists());
    }
}

#[cfg(unix)]
#[test]
fn symlink_alias_cannot_hide_a_provider_inside_the_worktree() {
    use std::os::unix::fs::symlink;
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let inside = workspace.path().join("objects");
    fs::create_dir(&inside).unwrap();
    let alias = outside.path().join("external-alias");
    symlink(&inside, &alias).unwrap();
    let result = run(
        workspace.path(),
        None,
        &["init", "--object-dir", alias.to_str().unwrap()],
    );
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("outside the worktree"));
    assert!(!workspace.path().join(".loom").exists());
    assert_eq!(fs::read_dir(inside).unwrap().count(), 0);
}
