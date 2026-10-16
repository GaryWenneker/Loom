use loom_core::{FilesystemObjectBackend, ObjectId, Repository, StatusKind};
use std::{env, path::Path, process};

fn usage() {
    eprintln!("Loom v0.1 (experimental)\n\nUsage:\n  loom init [path] [--object-dir DIRECTORY]\n  loom status\n  loom add PATH [PATH ...]\n  loom unstage PATH [PATH ...]\n  loom commit -m MESSAGE [--author NAME]\n  loom log [limit]\n  loom branch list|create NAME\n  loom switch NAME\n  loom merge NAME [-m MESSAGE]\n  loom merge-bases LEFT_BRANCH RIGHT_BRANCH\n  loom diff [--staged | --unstaged | FROM_BRANCH TO_BRANCH]\n  loom review\n  loom cat BRANCH PATH\n  loom cat @CHANGESET_ID PATH\n  loom files BRANCH|@CHANGESET_ID [PREFIX] [--null]\n  loom tree @CHANGESET_ID\n  loom tree ls @TREE_ID [DIR]\n  loom tree cat @TREE_ID PATH\n  loom fsck\n  loom git -- GIT_ARGS...\n\nExternal object storage: set LOOM_OBJECT_DIR for every command.\n");
}


fn print_diff(change: &loom_core::FileDiff) {
    let name = &change.path;
    println!("diff --loom a/{name} b/{name}");
    let before = change.before.as_deref().unwrap_or(&[]);
    let after = change.after.as_deref().unwrap_or(&[]);
    if before.len().saturating_add(after.len()) > 1_048_576 {
        println!("File contents exceed the 1 MiB text diff display limit");
        return;
    }
    match (std::str::from_utf8(before), std::str::from_utf8(after)) {
        (Ok(old), Ok(new)) if !old.contains('\0') && !new.contains('\0') => {
            let old_name = if change.before.is_some() { format!("a/{name}") } else { "/dev/null".into() };
            let new_name = if change.after.is_some() { format!("b/{name}") } else { "/dev/null".into() };
            let patch = similar::TextDiff::from_lines(old, new);
            print!("{}", patch.unified_diff().context_radius(3).header(&old_name, &new_name));
        }
        _ => println!("Binary files differ"),
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else { usage(); return Err("missing command".into()); };
    if matches!(cmd, "--help" | "-h" | "help") { usage(); return Ok(()); }
    if cmd == "init" {
        let mut path = None;
        let mut object_dir = env::var_os("LOOM_OBJECT_DIR");
        let mut rest = args[1..].iter();
        while let Some(arg) = rest.next() {
            if arg == "--object-dir" {
                object_dir = Some(rest.next().ok_or("--object-dir requires a directory")?.into());
            } else if arg.starts_with('-') || path.is_some() {
                return Err("usage: loom init [path] [--object-dir DIRECTORY]".into());
            } else {
                path = Some(arg.as_str());
            }
        }
        let path = path.unwrap_or(".");
        if !Path::new(path).exists() { std::fs::create_dir_all(path)?; }
        let repo = if let Some(dir) = object_dir {
            Repository::init_with_object_backend(
                path, std::sync::Arc::new(FilesystemObjectBackend::open(dir)?),
            )?
        } else {
            Repository::init(path)?
        };
        println!("Initialized native Loom repository at {}", repo.root().display());
        return Ok(());
    }
    let repo = if let Some(dir) = env::var_os("LOOM_OBJECT_DIR") {
        Repository::discover_with_object_backend(
            env::current_dir()?, std::sync::Arc::new(FilesystemObjectBackend::open(dir)?),
        )?
    } else {
        Repository::discover(env::current_dir()?)?
    };
    match cmd {
        "status" => {
            println!("On branch {}", repo.current_branch()?);
            let staged = repo.staged_status()?;
            if repo.staging_enabled()? {
                println!("Explicit staging enabled");
                if staged.is_empty() { println!("No changes staged"); }
                for change in staged {
                    let flag = match change.kind {
                        StatusKind::Added => "A", StatusKind::Modified => "M", StatusKind::Deleted => "D"
                    };
                    println!("staged {flag} {}", change.path);
                }
            }
            let changes = repo.unstaged_status()?;
            if changes.is_empty() { println!("Working tree clean"); }
            for change in changes {
                let flag = match change.kind { StatusKind::Added => "A", StatusKind::Modified => "M", StatusKind::Deleted => "D" };
                println!("{flag} {}", change.path);
            }
        }
        "add" | "unstage" => {
            if args.len() < 2 {
                return Err(format!("usage: loom {cmd} PATH [PATH ...]").into());
            }
            for path in &args[1..] {
                if cmd == "add" { repo.add(path)?; }
                else { repo.unstage(path)?; }
                println!("{} {path}", if cmd == "add" { "Staged" } else { "Unstaged" });
            }
        }
        "commit" => {
            let pos = args.iter().position(|x| x == "-m").ok_or("commit requires -m MESSAGE")?;
            let msg = args.get(pos+1).ok_or("commit requires a message after -m")?;
            let author = if let Some(pos) = args.iter().position(|x| x == "--author") {
                args.get(pos+1).ok_or("missing --author value")?.clone()
            } else {
                env::var("LOOM_AUTHOR").or_else(|_| env::var("USER")).or_else(|_| env::var("USERNAME")).unwrap_or_else(|_| "unknown".into())
            };
            let oid = repo.commit(&author, msg)?;
            println!("Committed {oid}");
        }
        "log" => {
            let limit = args.get(1).map(|s| s.parse::<usize>()).transpose()?.unwrap_or(20);
            for (oid, c) in repo.log(limit)? {
                println!("commit {oid}\nAuthor: {}\nDate: {}\n\n    {}\n", c.author, c.timestamp_seconds, c.message);
            }
        }
        "branch" => match args.get(1).map(String::as_str) {
            Some("list") => {
                let current = repo.current_branch()?;
                for (name, tip) in repo.branches()? {
                    println!("{} {} {}", if name == current { "*" } else { " " }, name,
                        tip.map(|x| x.to_string()).unwrap_or_else(|| "(unborn)".into()));
                }
            }
            Some("create") => {
                let name = args.get(2).ok_or("branch create requires NAME")?;
                repo.create_branch(name)?;
                println!("Created branch {name}");
            }
            _ => return Err("usage: loom branch list|create NAME".into())
        },
        "switch" => {
            let name = args.get(1).ok_or("switch requires a branch name")?;
            repo.switch_branch(name)?;
            println!("Switched to branch {name}");
        }
        "merge" => {
            let name = args.get(1).ok_or("merge requires a source branch")?;
            let message = if let Some(pos) = args.iter().position(|x| x == "-m") {
                args.get(pos+1).ok_or("missing -m value")?.clone()
            } else { format!("Merge branch {name}") };
            let author = env::var("LOOM_AUTHOR").or_else(|_| env::var("USER"))
                .or_else(|_| env::var("USERNAME")).unwrap_or_else(|_| "unknown".into());
            let before = repo.head()?;
            let source_tip = repo.branch_tip(name)?;
            let oid = repo.merge_branch(name, &author, &message)?;
            if before == Some(oid) {
                println!("Already up to date with {name} ({oid})");
            } else if source_tip == Some(oid) {
                println!("Fast-forwarded to {name} ({oid})");
            } else {
                println!("Merged {name} as {oid}");
            }
        }
        "merge-bases" => {
            if args.len() != 3 {
                return Err("usage: loom merge-bases LEFT_BRANCH RIGHT_BRANCH".into());
            }
            for base in repo.branch_merge_bases(&args[1], &args[2])? {
                println!("{base}");
            }
        }
        "diff" => {
            let changes = match args.as_slice() {
                [_] => repo.diff_worktree()?,
                [_, option] if option == "--staged" => repo.diff_staged()?,
                [_, option] if option == "--unstaged" => repo.diff_unstaged()?,
                [_, from, to] => repo.diff_branches(from, to)?,
                _ => return Err("usage: loom diff [--staged | --unstaged | FROM_BRANCH TO_BRANCH]".into()),
            };
            for change in &changes { print_diff(change); }
        }
        "review" => {
            if args.len() != 1 { return Err("usage: loom review".into()); }
            println!("=== Ready to commit (staged) ===");
            let staged = repo.diff_staged()?;
            if staged.is_empty() { println!("No staged changes"); }
            for change in &staged { print_diff(change); }
            println!("=== Working changes (not staged) ===");
            let working = repo.diff_unstaged()?;
            if working.is_empty() { println!("No unstaged changes"); }
            for change in &working { print_diff(change); }
            if !repo.staging_enabled()? {
                println!("Legacy commit-all mode: 'loom commit' currently includes all working changes.");
            }
        }
        "files" => {
            // Text mode quotes/escapes filenames, including embedded newlines.
            // --null uses NUL-delimited raw UTF-8 paths for tool pipelines.
            let null = args.last().is_some_and(|value| value == "--null");
            let positional = if null { &args[1..args.len()-1] } else { &args[1..] };
            let (branch, prefix) = match positional {
                [branch] => (branch.as_str(), None),
                [branch, prefix] => (branch.as_str(), Some(prefix.as_str())),
                _ => return Err("usage: loom files BRANCH|@CHANGESET_ID [PREFIX] [--null]".into()),
            };
            let files = if let Some(hex) = branch.strip_prefix('@') {
                repo.list_revision_files(ObjectId::parse(hex)?, prefix)?
            } else {
                repo.list_branch_files(branch, prefix)?
            };
            use std::io::Write;
            let stdout = std::io::stdout();
            let mut output = stdout.lock();
            for path in files {
                if null {
                    output.write_all(path.as_bytes())?;
                    output.write_all(&[0])?;
                } else {
                    writeln!(output, "{path:?}")?;
                }
            }
        }
        "tree" => {
            match args.as_slice() {
                [_, revision] if revision.starts_with('@') => {
                    let root = repo.native_tree_root(
                        ObjectId::parse(&revision[1..])?
                    )?;
                    println!("{root}");
                }
                [_, action, root] if action == "ls" => {
                    let hex = root.strip_prefix('@')
                        .ok_or("tree ls requires a @TREE_ID")?;
                    let entries = repo.native_tree_entries(
                        ObjectId::parse(hex)?, None
                    )?;
                    for entry in entries {
                        let kind = match entry.kind {
                            loom_core::TreeEntryKind::File => "file",
                            loom_core::TreeEntryKind::Directory => "dir",
                        };
                        println!("{kind} {} {:?}", entry.object, entry.name);
                    }
                }
                [_, action, root, directory] if action == "ls" => {
                    let hex = root.strip_prefix('@')
                        .ok_or("tree ls requires a @TREE_ID")?;
                    let entries = repo.native_tree_entries(
                        ObjectId::parse(hex)?, Some(directory)
                    )?;
                    for entry in entries {
                        let kind = match entry.kind {
                            loom_core::TreeEntryKind::File => "file",
                            loom_core::TreeEntryKind::Directory => "dir",
                        };
                        println!("{kind} {} {:?}", entry.object, entry.name);
                    }
                }
                [_, action, root, path] if action == "cat" => {
                    let hex = root.strip_prefix('@')
                        .ok_or("tree cat requires a @TREE_ID")?;
                    let data = repo.native_tree_file(
                        ObjectId::parse(hex)?, path
                    )?.ok_or_else(|| format!("not a file in native tree: {path}"))?;
                    use std::io::Write;
                    std::io::stdout().lock().write_all(&data)?;
                }
                _ => return Err(
                    "usage: loom tree @CHANGESET_ID | ls @TREE_ID [DIR] | cat @TREE_ID PATH".into()
                ),
            }
        }
        "cat" => {
            if args.len() != 3 { return Err("usage: loom cat BRANCH|@CHANGESET_ID PATH".into()); }
            let data = if let Some(hex) = args[1].strip_prefix('@') {
                repo.read_revision_file(ObjectId::parse(hex)?, &args[2])?
            } else {
                repo.read_branch_file(&args[1], &args[2])?
            }.ok_or_else(|| format!("path not found at {}: {}", args[1], args[2]))?;
            use std::io::Write;
            std::io::stdout().lock().write_all(&data)?;
        }
        "fsck" => println!("Verified {} native objects and branch references", repo.fsck()?),
        _ => { usage(); return Err(format!("unknown command: {cmd}").into()); }
    }
    Ok(())
}
/// Run the explicit bridge before UTF-8 native argument parsing or discovery.
/// OS arguments preserve binary filenames and are passed directly, without a shell.
fn git_bridge(args: &[std::ffi::OsString]) -> Result<i32, Box<dyn std::error::Error>> {
    if args.get(1).map(|s| s.as_os_str()) != Some(std::ffi::OsStr::new("--")) || args.len() < 3 {
        return Err("usage: loom git -- GIT_ARGS...".into());
    }
    let status = process::Command::new("git").args(&args[2..]).status()
        .map_err(|error| format!("cannot launch Git compatibility bridge: {error}"))?;
    if let Some(code) = status.code() { return Ok(code); }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        return Ok(128 + status.signal().unwrap_or(1));
    }
    #[cfg(not(unix))]
    Ok(1)
}

fn main() {
    let raw: Vec<_> = env::args_os().skip(1).collect();
    if raw.first().map(|s| s.as_os_str()) == Some(std::ffi::OsStr::new("git")) {
        match git_bridge(&raw) {
            Ok(code) => process::exit(code),
            Err(error) => { eprintln!("loom: {error}"); process::exit(1); },
        }
    }
    if let Err(e) = run() {
        eprintln!("loom: {e}");
        process::exit(1);
    }
}
