use std::{fs, path::Path, process::Command};

fn call(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root)
        .env("LOOM_AUTHOR", "merge-base-test")
        .args(args)
        .output()
        .expect("run Loom CLI")
}

fn ok(root: &Path, args: &[&str]) -> String {
    let result = call(root, args);
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn command_lists_dag_best_bases_without_touching_worktree() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::write(root.join("shared"), b"initial").unwrap();
    let initial = ok(root, &["commit", "-m", "root"]);
    let initial = initial.trim().strip_prefix("Committed ").unwrap().to_owned();
    ok(root, &["branch", "create", "feature"]);

    ok(root, &["switch", "feature"]);
    fs::write(root.join("feature"), b"feature").unwrap();
    ok(root, &["commit", "-m", "feature"]);
    ok(root, &["switch", "main"]);
    fs::write(root.join("main-only"), b"main").unwrap();
    ok(root, &["commit", "-m", "main"]);

    assert_eq!(ok(root, &["merge-bases", "main", "feature"]),
               format!("{initial}\n"));
    assert_eq!(ok(root, &["merge-bases", "feature", "main"]),
               format!("{initial}\n"));
    assert!(!call(root, &["merge-bases", "main"]).status.success());
    assert!(!call(root, &["merge-bases", "unknown", "main"]).status.success());
    assert_eq!(fs::read(root.join("main-only")).unwrap(), b"main");
    assert!(!root.join("feature").exists());

    let merge = ok(root, &["merge", "feature", "-m", "merge"]);
    assert!(merge.contains("Merged feature as "));
    let feature_tip = ok(root, &["merge-bases", "feature", "feature"]);
    assert_eq!(ok(root, &["merge-bases", "main", "feature"]), feature_tip);
    ok(root, &["fsck"]);
}
