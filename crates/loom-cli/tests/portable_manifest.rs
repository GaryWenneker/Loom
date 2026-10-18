use std::{fs, path::Path, process::Command};

fn call(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env("LOOM_AUTHOR", "portable-manifest-test")
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
fn portable_manifest_is_typed_idempotent_and_preserves_repository_state() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    ok(root, &["init"]);
    fs::write(root.join("tracked"), b"committed").unwrap();
    ok(root, &["commit", "-m", "base"]);
    fs::write(root.join("tracked"), b"unstaged").unwrap();
    fs::write(root.join("staged"), b"staged").unwrap();
    ok(root, &["add", "staged"]);
    fs::write(root.join("payload"), b"portable bytes").unwrap();
    let status_before = ok(root, &["status"]);

    let regular = ok(root, &["portable-manifest", "regular", "payload"]);
    let retry = ok(root, &["portable-manifest", "regular", "payload"]);
    let executable = ok(root, &["portable-manifest", "executable", "payload"]);
    assert_eq!(regular, retry);
    assert_ne!(regular, executable);
    assert_eq!(ok(root, &["status"]), status_before);
    ok(root, &["fsck"]);
}

#[test]
fn portable_manifest_streams_files_beyond_the_legacy_cli_cap() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    ok(root, &["init"]);
    let payload = fs::File::create(root.join("large-payload")).unwrap();
    payload.set_len(64 * 1024 * 1024 + 1).unwrap();

    let id = ok(root, &["portable-manifest", "regular", "large-payload"]);
    assert_eq!(String::from_utf8(id).unwrap().trim().len(), 64);
    ok(root, &["fsck"]);
}

#[test]
fn portable_tree_is_canonical_typed_and_repository_independent() {
    let temporary = tempfile::tempdir().unwrap();
    let exports = tempfile::tempdir().unwrap();
    let root = temporary.path();
    ok(root, &["init"]);
    fs::write(root.join("tracked"), b"committed").unwrap();
    ok(root, &["commit", "-m", "base"]);
    fs::write(root.join("tracked"), b"unstaged").unwrap();
    fs::create_dir(root.join("portable")).unwrap();
    fs::create_dir(root.join("portable/sub")).unwrap();
    fs::write(root.join("portable/a"), b"alpha").unwrap();
    fs::write(root.join("portable/sub/b"), b"beta").unwrap();
    let status_before = ok(root, &["status"]);

    let first = ok(root, &["portable-tree", "portable", "--workers", "1"]);
    let retry = ok(root, &["portable-tree", "portable", "--workers", "8"]);
    assert_eq!(first, retry);
    let tree = String::from_utf8(first).unwrap();
    assert_eq!(tree.trim().len(), 64);
    let immutable = format!("@{}", tree.trim());
    let serial = exports.path().join("serial");
    let parallel = exports.path().join("parallel");
    ok(
        root,
        &[
            "portable-export",
            &immutable,
            serial.to_str().unwrap(),
            "--workers",
            "1",
        ],
    );
    ok(
        root,
        &[
            "portable-export",
            &immutable,
            parallel.to_str().unwrap(),
            "--workers",
            "8",
        ],
    );
    assert_eq!(fs::read(serial.join("a")).unwrap(), b"alpha");
    assert_eq!(fs::read(parallel.join("sub/b")).unwrap(), b"beta");
    assert_eq!(
        fs::read(serial.join(".loom-portable-complete")).unwrap(),
        fs::read(parallel.join(".loom-portable-complete")).unwrap()
    );
    assert!(!call(
        root,
        &["portable-export", &immutable, serial.to_str().unwrap()]
    )
    .status
    .success());
    assert_eq!(ok(root, &["status"]), status_before);
    assert!(
        !call(root, &["portable-tree", "portable", "--workers", "bad"])
            .status
            .success()
    );
    ok(root, &["fsck"]);
}

#[cfg(target_os = "linux")]
#[test]
fn portable_tree_rejects_case_collisions() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    ok(root, &["init"]);
    fs::create_dir(root.join("portable")).unwrap();
    fs::write(root.join("portable/A"), b"one").unwrap();
    fs::write(root.join("portable/a"), b"two").unwrap();
    assert!(!call(root, &["portable-tree", "portable"]).status.success());
}

#[cfg(unix)]
#[test]
fn portable_symlink_manifest_rejects_escape_targets() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    ok(root, &["init"]);
    symlink("target", root.join("safe-link")).unwrap();
    ok(root, &["portable-manifest", "symlink", "safe-link"]);
    symlink("../escape", root.join("escape-link")).unwrap();
    assert!(
        !call(root, &["portable-manifest", "symlink", "escape-link"])
            .status
            .success()
    );
}
