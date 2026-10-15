use loom_core::{Repository, StatusKind};
use std::{env, path::Path, process};

fn usage() {
    eprintln!("Loom v0.1 (experimental)\n\nUsage:\n  loom init [path]\n  loom status\n  loom commit -m MESSAGE [--author NAME]\n  loom log [limit]\n  loom branch list|create NAME\n  loom switch NAME\n  loom merge NAME [-m MESSAGE]\n  loom fsck\n");
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
            let changes = repo.status()?;
            if changes.is_empty() { println!("Working tree clean"); }
            for change in changes {
                let flag = match change.kind { StatusKind::Added => "A", StatusKind::Modified => "M", StatusKind::Deleted => "D" };
                println!("{flag} {}", change.path);
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
            let changes = match args.len() {
                1 => repo.diff_worktree()?,
                3 => repo.diff_branches(&args[1], &args[2])?,
                _ => return Err("usage: loom diff [FROM_BRANCH TO_BRANCH]".into()),
            };
            for change in &changes { print_diff(change); }
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
