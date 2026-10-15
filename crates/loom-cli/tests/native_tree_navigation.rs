use std::{fs, path::Path, process::Command};

fn call(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .current_dir(root).env("LOOM_AUTHOR", "tree-navigation-test")
        .args(args).output().unwrap()
}

fn ok(root: &Path, args: &[&str]) -> Vec<u8> {
    let result = call(root, args);
    assert!(result.status.success(),
        "{args:?}: {}", String::from_utf8_lossy(&result.stderr));
    result.stdout
}

#[test]
fn list_and_read_nested_native_trees_without_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init"]);
    fs::create_dir_all(root.join("src/deep")).unwrap();
    fs::write(root.join("src/deep/data.bin"), [0, 255, 10, 0]).unwrap();
    fs::write(root.join("README.md"), b"readme").unwrap();
    let raw_commit = ok(root, &["commit", "-m", "initial"]);
    let commit = std::str::from_utf8(&raw_commit).unwrap().trim()
        .strip_prefix("Committed ").unwrap().to_owned();
    let tree = ok(root, &["tree", &format!("@{commit}")]);
    let root_id = String::from_utf8(tree).unwrap();
    let reference = format!("@{}", root_id.trim());

    let top = String::from_utf8(ok(root, &["tree", "ls", &reference])).unwrap();
    assert!(top.contains("\"README.md\""), "{top}");
    assert!(top.contains("dir "), "{top}");
    assert!(top.contains("\"src\""), "{top}");
    let nested = String::from_utf8(
        ok(root, &["tree", "ls", &reference, "src/deep"])
    ).unwrap();
    assert!(nested.contains("\"data.bin\""), "{nested}");
    assert_eq!(ok(root, &["tree", "cat", &reference, "src/deep/data.bin"]),
               [0, 255, 10, 0]);
    assert_eq!(ok(root, &["tree", "cat", &reference, "README.md"]), b"readme");

    // Changing the worktree cannot alter the bytes of the published tree.
    fs::write(root.join("src/deep/data.bin"), b"changed").unwrap();
    assert_eq!(ok(root, &["tree", "cat", &reference, "src/deep/data.bin"]),
               [0, 255, 10, 0]);
    assert!(!call(root, &["tree", "cat", &reference, "missing"]).status.success());
    assert!(!call(root, &["tree", "cat", &reference, "src"]).status.success());
    assert!(!call(root, &["tree", "ls", &reference, "README.md"]).status.success());
    assert!(!call(root, &["tree", "ls", "@invalid"]).status.success());
    ok(root, &["fsck"]);
}
