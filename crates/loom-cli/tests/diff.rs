use std::{fs, path::Path, process::Command};

fn run(root: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env("LOOM_AUTHOR", "diff-test")
        .args(args)
        .output()
        .expect("run loom");
    assert!(
        output.status.success(),
        "loom {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn worktree_diff_uses_unified_patch_and_is_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    run(root, &["init"]);
    fs::write(root.join("text.txt"), "first\nsecond\n").unwrap();
    run(root, &["commit", "-m", "baseline"]);
    fs::write(root.join("text.txt"), "first\nchanged\n").unwrap();
    let patch = run(root, &["diff"]);
    assert!(patch.contains("--- a/text.txt"), "{patch}");
    assert!(patch.contains("+++ b/text.txt"), "{patch}");
    assert!(patch.contains("-second"), "{patch}");
    assert!(patch.contains("+changed"), "{patch}");
    assert!(run(root, &["status"]).contains("M text.txt"));
    fs::write(root.join("new.txt"), "added\n").unwrap();
    let added = run(root, &["diff"]);
    assert!(added.contains("+++ b/new.txt"), "{added}");
    assert!(added.contains("--- /dev/null"), "{added}");
    assert!(root.join("new.txt").exists());
}

#[test]
fn branch_diff_does_not_switch_or_modify_worktree() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    run(root, &["init"]);
    fs::write(root.join("a"), "base\n").unwrap();
    run(root, &["commit", "-m", "base"]);
    run(root, &["branch", "create", "feature"]);
    run(root, &["switch", "feature"]);
    fs::write(root.join("a"), "feature\n").unwrap();
    fs::write(root.join("binary"), [0u8, 255u8]).unwrap();
    run(root, &["commit", "-m", "feature"]);
    run(root, &["switch", "main"]);
    let patch = run(root, &["diff", "main", "feature"]);
    assert!(patch.contains("+feature"), "{patch}");
    assert!(patch.contains("Binary files differ"), "{patch}");
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "base\n");
    assert!(!root.join("binary").exists());
    assert!(run(root, &["status"]).contains("Working tree clean"));
    let invalid = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .args(["diff", "missing", "main"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
}

#[test]
fn cat_reads_snapshot_without_switching_branches_or_losing_binary_data() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    run(root, &["init"]);
    fs::write(root.join("data.bin"), [0, 1, 255, 2]).unwrap();
    run(root, &["commit", "-m", "binary"]);
    run(root, &["branch", "create", "release"]);
    fs::write(root.join("data.bin"), [0, 3, 4, 5]).unwrap();
    run(root, &["commit", "-m", "update"]);
    let output = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .args(["cat", "release", "data.bin"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.stdout, [0, 1, 255, 2]);
    assert_eq!(fs::read(root.join("data.bin")).unwrap(), [0, 3, 4, 5]);
    let invalid = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .args(["cat", "release", "../data.bin"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
}

#[test]
fn merges_nonoverlapping_text_changes_in_same_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    run(root, &["init"]);
    fs::write(root.join("shared.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    run(root, &["commit", "-m", "base"]);
    run(root, &["branch", "create", "feature"]);
    run(root, &["switch", "feature"]);
    fs::write(root.join("shared.txt"), "ONE\ntwo\nthree\nfour\n").unwrap();
    run(root, &["commit", "-m", "change first"]);
    run(root, &["switch", "main"]);
    fs::write(root.join("shared.txt"), "one\ntwo\nthree\nFOUR\n").unwrap();
    run(root, &["commit", "-m", "change last"]);
    run(root, &["merge", "feature"]);
    assert_eq!(
        fs::read_to_string(root.join("shared.txt")).unwrap(),
        "ONE\ntwo\nthree\nFOUR\n"
    );
    assert!(run(root, &["fsck"]).contains("Verified"));
    assert!(run(root, &["status"]).contains("Working tree clean"));
}
