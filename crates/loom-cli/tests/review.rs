use std::{fs, path::Path, process::Command};

fn exec(root: &Path, args: &[&str]) -> String {
    let result = Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env("LOOM_AUTHOR", "review-test")
        .args(args)
        .output().expect("run loom");
    assert!(result.status.success(),
        "{args:?} failed: {}", String::from_utf8_lossy(&result.stderr));
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn staged_snapshots_and_later_edits_are_independently_visible() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    exec(root, &["init"]);
    fs::write(root.join("story.txt"), b"base\n").unwrap();
    exec(root, &["commit", "-m", "base"]);

    fs::write(root.join("story.txt"), b"staged\n").unwrap();
    exec(root, &["add", "story.txt"]);
    let staged_unchanged = exec(root, &["status"]);
    assert!(staged_unchanged.contains("staged M story.txt"), "{staged_unchanged}");
    assert!(!staged_unchanged.contains("\nM story.txt"), "{staged_unchanged}");

    fs::write(root.join("story.txt"), b"working\n").unwrap();
    let status = exec(root, &["status"]);
    assert!(status.contains("staged M story.txt"), "{status}");
    assert!(status.contains("\nM story.txt"), "{status}");

    let staged = exec(root, &["diff", "--staged"]);
    assert!(staged.contains("-base"), "{staged}");
    assert!(staged.contains("+staged"), "{staged}");
    assert!(!staged.contains("+working"), "{staged}");

    let unstaged = exec(root, &["diff", "--unstaged"]);
    assert!(unstaged.contains("-staged"), "{unstaged}");
    assert!(unstaged.contains("+working"), "{unstaged}");
    assert!(!unstaged.contains("-base"), "{unstaged}");

    let all = exec(root, &["diff"]);
    assert!(all.contains("-base"), "{all}");
    assert!(all.contains("+working"), "{all}");

    let review = exec(root, &["review"]);
    assert!(review.contains("Ready to commit (staged)"), "{review}");
    assert!(review.contains("Working changes (not staged)"), "{review}");
    assert!(review.contains("+staged") && review.contains("+working"), "{review}");

    // Pure read paths cannot alter stage or worktree.
    exec(root, &["commit", "-m", "capture frozen staged version"]);
    assert_eq!(exec(root, &["cat", "main", "story.txt"]), "staged\n");
    assert_eq!(fs::read(root.join("story.txt")).unwrap(), b"working\n");
    assert!(exec(root, &["status"]).contains("No changes staged"));
}

#[test]
fn untouched_new_staged_file_is_not_double_reported() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    exec(root, &["init"]);
    fs::write(root.join("staged.txt"), "ready").unwrap();
    fs::write(root.join("untracked.txt"), "later").unwrap();
    exec(root, &["add", "staged.txt"]);
    let status = exec(root, &["status"]);
    assert!(status.contains("staged A staged.txt"), "{status}");
    assert!(!status.contains("\nA staged.txt"), "{status}");
    assert!(status.contains("\nA untracked.txt"), "{status}");
    assert!(!exec(root, &["diff", "--unstaged"]).contains("staged.txt"));
    assert!(exec(root, &["diff", "--staged"]).contains("staged.txt"));
}

#[test]
fn staged_deletion_and_reintroduced_file_are_separate_deltas() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    exec(root, &["init"]);
    fs::write(root.join("file"), "old").unwrap();
    exec(root, &["commit", "-m", "initial"]);
    fs::remove_file(root.join("file")).unwrap();
    exec(root, &["add", "file"]);
    assert!(exec(root, &["diff", "--staged"]).contains("--- a/file"));
    assert!(!exec(root, &["diff", "--unstaged"]).contains("diff --loom"));
    fs::write(root.join("file"), "restored").unwrap();
    let unstaged = exec(root, &["diff", "--unstaged"]);
    assert!(unstaged.contains("--- /dev/null"), "{unstaged}");
    assert!(unstaged.contains("+++ b/file"), "{unstaged}");
}
