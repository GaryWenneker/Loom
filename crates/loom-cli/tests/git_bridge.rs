use std::{fs, path::Path, process::Command};

fn bridge(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env_remove("LOOM_OBJECT_DIR")
        .env_remove("LOOM_OBJECT_DB")
        .args(args)
        .output()
        .unwrap()
}
fn git(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}
fn setup(root: &Path) {
    assert!(git(root, &["init", "-q"]).status.success());
    assert!(git(root, &["config", "user.name", "Loom Test"])
        .status
        .success());
    assert!(git(root, &["config", "user.email", "loom@example.invalid"])
        .status
        .success());
}
#[test]
fn git_only_repository_works_without_native_metadata() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path());
    fs::write(dir.path().join("space and $literal.txt"), b"git bytes").unwrap();
    assert!(bridge(
        dir.path(),
        &["git", "--", "add", "--", "space and $literal.txt"]
    )
    .status
    .success());
    assert!(bridge(
        dir.path(),
        &[
            "git",
            "--",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "native independence"
        ]
    )
    .status
    .success());
    let result = bridge(
        dir.path(),
        &["git", "--", "show", "HEAD:space and $literal.txt"],
    );
    assert!(result.status.success());
    assert_eq!(result.stdout, b"git bytes");
    assert!(!dir.path().join(".loom").exists());
    assert!(!bridge(dir.path(), &["status"]).status.success());
}
#[test]
fn git_diff_exit_codes_and_binary_output_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path());
    fs::write(dir.path().join("file"), [0, 1, 255, 10]).unwrap();
    assert!(git(dir.path(), &["add", "file"]).status.success());
    assert!(git(
        dir.path(),
        &["-c", "commit.gpgsign=false", "commit", "-qm", "base"]
    )
    .status
    .success());
    let raw = bridge(dir.path(), &["git", "--", "show", "HEAD:file"]);
    assert_eq!(raw.stdout, [0, 1, 255, 10]);
    fs::write(dir.path().join("file"), b"changed").unwrap();
    let direct = git(dir.path(), &["diff", "--exit-code"]);
    let forwarded = bridge(dir.path(), &["git", "--", "diff", "--exit-code"]);
    assert_eq!(forwarded.status.code(), Some(1));
    assert_eq!(forwarded.status.code(), direct.status.code());
    assert_eq!(forwarded.stdout, direct.stdout);
    let direct = git(dir.path(), &["rev-parse", "--verify", "refs/heads/missing"]);
    let forwarded = bridge(
        dir.path(),
        &["git", "--", "rev-parse", "--verify", "refs/heads/missing"],
    );
    assert_eq!(forwarded.status.code(), Some(128));
    assert_eq!(forwarded.status.code(), direct.status.code());
}
#[test]
fn explicit_separator_and_installed_git_are_required() {
    let dir = tempfile::tempdir().unwrap();
    for args in [vec!["git"], vec!["git", "status"], vec!["git", "--"]] {
        let result = bridge(dir.path(), &args);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("usage: loom git"));
    }
    let result = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(dir.path())
        .env("PATH", dir.path())
        .args(["git", "--", "--version"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("cannot launch Git compatibility bridge")
    );
    assert!(!dir.path().join(".loom").exists());
}
#[cfg(target_os = "linux")]
#[test]
fn non_utf8_git_paths_are_forwarded_losslessly() {
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path());
    let name = std::ffi::OsString::from_vec(vec![b'f', 0xff]);
    fs::write(dir.path().join(&name), b"bytes").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(dir.path())
        .args(["git", "--", "add", "--"])
        .arg(&name)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result = bridge(dir.path(), &["git", "--", "ls-files", "-z"]);
    assert_eq!(result.stdout, vec![b'f', 0xff, 0]);
}

#[cfg(unix)]
#[test]
fn non_utf8_arguments_are_forwarded_without_filename_assumptions() {
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path());
    let value = std::ffi::OsString::from_vec(vec![b'v', 0xff]);
    let result = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(dir.path())
        .args(["git", "--", "config", "--local", "test.raw"])
        .arg(value)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result = bridge(
        dir.path(),
        &["git", "--", "config", "--local", "--get", "test.raw"],
    );
    assert!(result.status.success());
    assert_eq!(result.stdout, vec![b'v', 0xff, b'\n']);
}
