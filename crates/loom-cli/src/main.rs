use loom_core::{ObjectId, Repository, StatusKind};
use std::{env, path::Path, process};

fn usage() {
    eprintln!("Loom v0.1 (experimental)\n\nUsage:\n  loom init [path]\n  loom status\n  loom add PATH [PATH ...]\n  loom unstage PATH [PATH ...]\n  loom commit -m MESSAGE [--author NAME]\n  loom log [limit]\n  loom branch list|create NAME\n  loom switch NAME\n  loom merge NAME [-m MESSAGE]\n  loom diff [--staged | --unstaged | FROM_BRANCH TO_BRANCH]\n  loom review\n  loom cat BRANCH PATH\n  loom cat @CHANGESET_ID PATH\n  loom files BRANCH [PREFIX] [--null]\n  loom fsck\n");
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
        let path = args.get(1).map_or(".", String::as_str);
        if !Path::new(path).exists() { std::fs::create_dir_all(path)?; }
        let repo = Repository::init(path)?;
        println!("Initialized native Loom repository at {}", repo.root().display());
        return Ok(());
    }
    let repo = Repository::discover(env::current_dir()?)?;
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
            let oid = repo.merge_branch(name, &author, &message)?;
            println!("Merged {name} as {oid}");
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
                _ => return Err("usage: loom files BRANCH [PREFIX] [--null]".into()),
            };
            let files = repo.list_branch_files(branch, prefix)?;
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
fn main() {
    if let Err(e) = run() {
        eprintln!("loom: {e}");
        process::exit(1);
    }
}
