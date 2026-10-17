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
