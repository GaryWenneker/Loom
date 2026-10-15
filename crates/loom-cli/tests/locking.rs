use fs2::FileExt;
use std::{
    fs::{self, OpenOptions},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn run(root: &Path, args: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env("LOOM_AUTHOR", "locking-test")
        .args(args)
        .output()
        .expect("run Loom CLI");
    assert!(
        output.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn separate_cli_process_waits_for_exclusive_worktree_lock() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    run(root, &["init"]);
    fs::write(root.join("file.txt"), "data\n").unwrap();

    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(".loom").join("workspace.lock"))
        .or_else(|_| {
            OpenOptions::new().read(true).write(true).create(true)
                .open(root.join(".loom").join("workspace.lock"))
        })
        .unwrap();
    lock.lock_exclusive().unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env("LOOM_AUTHOR", "locking-test")
        .args(["commit", "-m", "serialized"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    thread::sleep(Duration::from_millis(300));
    assert!(child.try_wait().unwrap().is_none(), "commit bypassed exclusive lock");

    FileExt::unlock(&lock).unwrap();
    drop(lock);

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "commit failed after lock released");
            break;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("child did not resume after worktree lock was released");
        }
        thread::sleep(Duration::from_millis(20));
    }
    run(root, &["fsck"]);
}
