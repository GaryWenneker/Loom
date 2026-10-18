mod batch;
use loom_core::{FilesystemObjectBackend, ObjectId, Repository, StatusKind, WorktreeScanOptions};
use std::{env, path::Path, process};

fn same_open_file(path: &std::fs::Metadata, opened: &std::fs::Metadata) -> bool {
    if !path.is_file()
        || path.file_type().is_symlink()
        || !opened.is_file()
        || opened.file_type().is_symlink()
        || path.len() != opened.len()
        || path.modified().ok() != opened.modified().ok()
    {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        path.dev() == opened.dev() && path.ino() == opened.ino()
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn portable_file_kind(_metadata: &std::fs::Metadata) -> loom_core::PortableFileKind {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if _metadata.permissions().mode() & 0o111 != 0 {
            return loom_core::PortableFileKind::Executable;
        }
    }
    loom_core::PortableFileKind::Regular
}

fn same_directory(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    if !before.is_dir()
        || !after.is_dir()
        || before.file_type().is_symlink()
        || after.file_type().is_symlink()
    {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        before.dev() == after.dev() && before.ino() == after.ino()
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn usage() {
    eprintln!("Loom v0.1 (experimental)\n\nUsage:\n  loom init [path] [--object-dir DIRECTORY | --object-db FILE]\n  loom status [--scan-workers N]\n  loom add [--scan-workers N] [--] PATH [PATH ...]\n  loom unstage PATH [PATH ...]\n  loom commit -m MESSAGE [--author NAME] [--scan-workers N]\n  loom log [limit]\n  loom branch list|create NAME\n  loom switch NAME\n  loom merge NAME [-m MESSAGE]\n  loom merge-bases LEFT_BRANCH RIGHT_BRANCH\n  loom diff [--staged | --unstaged | FROM_BRANCH TO_BRANCH]\n  loom review\n  loom cat BRANCH PATH\n  loom cat @CHANGESET_ID PATH\n  loom files BRANCH|@CHANGESET_ID [PREFIX] [--null]\n  loom tree @CHANGESET_ID\n  loom tree ls @TREE_ID [DIR]\n  loom tree cat @TREE_ID PATH\n  loom fsck\n  loom git -- GIT_ARGS...\n  loom query --batch @CHANGESET_ID [--cache-bytes N] [--read-pack FILE]\n  loom read-pack @CHANGESET_ID FILE\n\nExternal object storage: set LOOM_OBJECT_DIR or LOOM_OBJECT_DB for every command.\n");
    eprintln!("  loom sparse-export @CHANGESET_ID DESTINATION [PREFIX ...]");
    eprintln!("  loom portable-manifest regular|executable|symlink PATH");
    eprintln!("  loom portable-tree DIRECTORY [--workers N]");
    eprintln!("  loom portable-export @TREE_ID DESTINATION [--workers N]");
    eprintln!("  loom portable-verify @TREE_ID DIRECTORY [--workers N]");
    eprintln!("  loom portable-migrate-plan @CHANGESET_ID [--workers N]");
}

fn publish_portable_directory(
    repo: &Repository,
    directory: &Path,
    depth: usize,
    workers: usize,
) -> Result<ObjectId, Box<dyn std::error::Error>> {
    if depth > 256 {
        return Err("portable tree nesting limit reached".into());
    }
    let before = std::fs::symlink_metadata(directory)?;
    if !before.is_dir() || before.file_type().is_symlink() {
        return Err("portable-tree requires a non-symlink directory".into());
    }
    let mut discovered = Vec::new();
    for item in std::fs::read_dir(directory)? {
        let item = item?;
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| "portable tree names must be UTF-8")?;
        discovered.push((name, item.path()));
    }
    discovered.sort_by(|left, right| left.0.cmp(&right.0));
    let names_before: Vec<_> = discovered.iter().map(|(name, _)| name.clone()).collect();
    let mut entries: Vec<Option<loom_core::PortableTreeEntry>> = std::iter::repeat_with(|| None)
        .take(discovered.len())
        .collect();
    let mut files = Vec::new();
    let mut file_slots = Vec::new();
    for (slot, (name, path)) in discovered.into_iter().enumerate() {
        let metadata = std::fs::symlink_metadata(&path)?;
        let (kind, object) = if metadata.file_type().is_symlink() {
            let target_path = std::fs::read_link(&path)?;
            let target = target_path
                .to_str()
                .ok_or("portable symlink target must be UTF-8")?;
            let published = (
                loom_core::PortableTreeEntryKind::File,
                repo.publish_portable_blob(
                    loom_core::PortableFileKind::Symlink,
                    target.as_bytes(),
                )?,
            );
            if std::fs::read_link(&path)? != target_path {
                return Err("portable symlink changed while reading".into());
            }
            published
        } else if metadata.is_dir() {
            (
                loom_core::PortableTreeEntryKind::Directory,
                publish_portable_directory(repo, &path, depth + 1, workers)?,
            )
        } else if metadata.is_file() {
            files.push((portable_file_kind(&metadata), path));
            file_slots.push((slot, name));
            continue;
        } else {
            return Err(format!("unsupported portable tree entry: {}", path.display()).into());
        };
        entries[slot] = Some(loom_core::PortableTreeEntry { name, kind, object });
    }
    let file_ids = repo.publish_portable_files_from_paths(&files, workers)?;
    for ((slot, name), object) in file_slots.into_iter().zip(file_ids) {
        entries[slot] = Some(loom_core::PortableTreeEntry {
            name,
            kind: loom_core::PortableTreeEntryKind::File,
            object,
        });
    }
    let mut names_after = Vec::new();
    for item in std::fs::read_dir(directory)? {
        names_after.push(
            item?
                .file_name()
                .into_string()
                .map_err(|_| "portable tree names must be UTF-8")?,
        );
    }
    names_after.sort();
    let after = std::fs::symlink_metadata(directory)?;
    if names_after != names_before || !same_directory(&before, &after) {
        return Err("portable tree directory changed while reading".into());
    }
    let entries = entries
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or("portable tree entry preparation was incomplete")?;
    repo.publish_portable_tree(&loom_core::PortableTree { entries })
        .map_err(Into::into)
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
            let old_name = if change.before.is_some() {
                format!("a/{name}")
            } else {
                "/dev/null".into()
            };
            let new_name = if change.after.is_some() {
                format!("b/{name}")
            } else {
                "/dev/null".into()
            };
            let patch = similar::TextDiff::from_lines(old, new);
            print!(
                "{}",
                patch
                    .unified_diff()
                    .context_radius(3)
                    .header(&old_name, &new_name)
            );
        }
        _ => println!("Binary files differ"),
    }
}

fn scan_options(args: &[String]) -> Result<WorktreeScanOptions, Box<dyn std::error::Error>> {
    let positions: Vec<_> = args
        .iter()
        .enumerate()
        .filter(|(_, arg)| arg.as_str() == "--scan-workers")
        .map(|(pos, _)| pos)
        .collect();
    if positions.len() > 1 {
        return Err("duplicate --scan-workers".into());
    }
    let workers = positions
        .first()
        .map(|pos| {
            args.get(pos + 1)
                .ok_or("--scan-workers requires an integer")?
                .parse::<usize>()
                .map_err(|_| "--scan-workers requires a nonnegative integer")
        })
        .transpose()?;
    Ok(WorktreeScanOptions {
        workers: workers.unwrap_or(0),
    })
}

type ObjectProvider = std::sync::Arc<dyn loom_core::storage_backend::ObjectBackend>;

fn object_backend(
    directory: Option<std::ffi::OsString>,
    database: Option<std::ffi::OsString>,
    existing: bool,
) -> Result<Option<ObjectProvider>, Box<dyn std::error::Error>> {
    match (directory, database) {
        (Some(_), Some(_)) => {
            Err("choose one object provider: LOOM_OBJECT_DIR or LOOM_OBJECT_DB".into())
        }
        (Some(path), None) => Ok(Some(std::sync::Arc::new(if existing {
            FilesystemObjectBackend::open_existing(path)?
        } else {
            FilesystemObjectBackend::open(path)?
        }))),
        (None, Some(path)) => Ok(Some(std::sync::Arc::new(if existing {
            loom_core::sqlite_object_storage::SqliteObjectBackend::open_existing(path)?
        } else {
            loom_core::sqlite_object_storage::SqliteObjectBackend::open(path)?
        }))),
        (None, None) => Ok(None),
    }
}

/// Object-provider files must never become input to their own VCS snapshots.
fn validate_external_location(
    workspace: &Path,
    directory: Option<&std::ffi::OsStr>,
    database: Option<&std::ffi::OsStr>,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = std::fs::canonicalize(workspace)?;
    for value in [directory, database].into_iter().flatten() {
        let path = Path::new(value);
        let location = if path.exists() {
            std::fs::canonicalize(path)?
        } else {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let name = path
                .file_name()
                .ok_or("object storage requires a named path")?;
            std::fs::canonicalize(parent)?.join(name)
        };
        if location.starts_with(&root) {
            return Err("external object storage must be outside the worktree to prevent tracking its own files".into());
        }
    }
    Ok(())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        usage();
        return Err("missing command".into());
    };
    if matches!(cmd, "--help" | "-h" | "help") {
        usage();
        return Ok(());
    }
    if cmd == "init" {
        let mut path = None;
        let mut object_dir = env::var_os("LOOM_OBJECT_DIR");
        let mut object_db = env::var_os("LOOM_OBJECT_DB");
        let mut selected = false;
        let mut rest = args[1..].iter();
        while let Some(arg) = rest.next() {
            if arg == "--object-dir" || arg == "--object-db" {
                if selected {
                    return Err("choose exactly one object storage option".into());
                }
                selected = true;
                let value = rest.next().ok_or("object storage option requires a path")?;
                object_dir = if arg == "--object-dir" {
                    Some(value.into())
                } else {
                    None
                };
                object_db = if arg == "--object-db" {
                    Some(value.into())
                } else {
                    None
                };
            } else if arg.starts_with('-') || path.is_some() {
                return Err(
                    "usage: loom init [path] [--object-dir DIRECTORY | --object-db FILE]".into(),
                );
            } else {
                path = Some(arg.as_str());
            }
        }
        let path = path.unwrap_or(".");
        if !Path::new(path).exists() {
            std::fs::create_dir_all(path)?;
        }
        validate_external_location(Path::new(path), object_dir.as_deref(), object_db.as_deref())?;
        let backend = object_backend(object_dir, object_db, false)?;
        let repo = if let Some(backend) = backend {
            Repository::init_with_object_backend(path, backend)?
        } else {
            Repository::init(path)?
        };
        println!(
            "Initialized native Loom repository at {}",
            repo.root().display()
        );
        return Ok(());
    }
    if cmd == "query" {
        if args.len() < 3 {
            return Err(
                "usage: loom query --batch @CHANGESET_ID [--cache-bytes N] [--read-pack FILE]"
                    .into(),
            );
        }
        if args[1] != "--batch" {
            return Err("query requires --batch".into());
        }
        let revision = ObjectId::parse(
            args[2]
                .strip_prefix('@')
                .ok_or("query requires an immutable @CHANGESET_ID")?,
        )?;
        let mut budget = None;
        let mut read_pack = None;
        let mut options = args[3..].iter();
        while let Some(option) = options.next() {
            let value = options.next().ok_or("query option requires a value")?;
            match option.as_str() {
                "--cache-bytes" if budget.is_none() => budget = Some(value.parse::<usize>()?),
                "--read-pack" if read_pack.is_none() => read_pack = Some(Path::new(value)),
                _ => return Err("unknown or duplicate query option".into()),
            }
        }
        let backend = object_backend(
            env::var_os("LOOM_OBJECT_DIR"),
            env::var_os("LOOM_OBJECT_DB"),
            true,
        )?;
        let snapshot = Repository::query_snapshot_with_read_pack(
            env::current_dir()?,
            revision,
            backend,
            budget.unwrap_or(loom_core::DEFAULT_SNAPSHOT_CACHE_BYTES),
            read_pack,
        )?;
        batch::serve(
            &snapshot,
            std::io::stdin().lock(),
            std::io::BufWriter::with_capacity(64 * 1024, std::io::stdout().lock()),
        )?;
        return Ok(());
    }
    if cmd == "sparse-export" {
        if args.len() < 3 {
            return Err("usage: loom sparse-export @CHANGESET_ID DESTINATION [PREFIX ...]".into());
        }
        let revision = ObjectId::parse(
            args[1]
                .strip_prefix('@')
                .ok_or("sparse-export requires an immutable @CHANGESET_ID")?,
        )?;
        let backend = object_backend(
            env::var_os("LOOM_OBJECT_DIR"),
            env::var_os("LOOM_OBJECT_DB"),
            true,
        )?;
        let snapshot = Repository::query_snapshot(
            env::current_dir()?,
            revision,
            backend,
            loom_core::DEFAULT_SNAPSHOT_CACHE_BYTES,
        )?;
        let stats = snapshot.export_sparse(Path::new(&args[2]), &args[3..])?;
        println!(
            "Exported {} files ({} bytes) from {}",
            stats.files,
            stats.bytes,
            snapshot.root()
        );
        return Ok(());
    }
    let repo = if let Some(backend) = object_backend(
        env::var_os("LOOM_OBJECT_DIR"),
        env::var_os("LOOM_OBJECT_DB"),
        true,
    )? {
        Repository::discover_with_object_backend(env::current_dir()?, backend)?
    } else {
        Repository::discover(env::current_dir()?)?
    };
    match cmd {
        "read-pack" => {
            if args.len() != 3 {
                return Err("usage: loom read-pack @CHANGESET_ID FILE".into());
            }
            let revision = ObjectId::parse(
                args[1]
                    .strip_prefix('@')
                    .ok_or("read-pack requires an immutable @CHANGESET_ID")?,
            )?;
            let stats = repo.write_read_pack(revision, &args[2])?;
            println!(
                "Packed {} objects into {} bytes",
                stats.objects, stats.bytes
            );
        }
        "status" => {
            if args.len() != 1 && (args.len() != 3 || args[1] != "--scan-workers") {
                return Err("usage: loom status [--scan-workers N]".into());
            }
            let options = scan_options(&args)?;
            println!("On branch {}", repo.current_branch()?);
            let staged = repo.staged_status()?;
            if repo.staging_enabled()? {
                println!("Explicit staging enabled");
                if staged.is_empty() {
                    println!("No changes staged");
                }
                for change in staged {
                    let flag = match change.kind {
                        StatusKind::Added => "A",
                        StatusKind::Modified => "M",
                        StatusKind::Deleted => "D",
                    };
                    println!("staged {flag} {}", change.path);
                }
            }
            let changes = repo.unstaged_status_with_scan_options(options)?;
            if changes.is_empty() {
                println!("Working tree clean");
            }
            for change in changes {
                let flag = match change.kind {
                    StatusKind::Added => "A",
                    StatusKind::Modified => "M",
                    StatusKind::Deleted => "D",
                };
                println!("{flag} {}", change.path);
            }
        }
        "add" => {
            let mut workers = None;
            let mut paths = Vec::new();
            let mut rest = args[1..].iter();
            while let Some(arg) = rest.next() {
                if arg == "--" {
                    paths.extend(rest.map(String::as_str));
                    break;
                } else if arg == "--scan-workers" {
                    if workers.is_some() {
                        return Err("duplicate --scan-workers".into());
                    }
                    let value = rest.next().ok_or("--scan-workers requires an integer")?;
                    workers = Some(
                        value
                            .parse::<usize>()
                            .map_err(|_| "--scan-workers requires a nonnegative integer")?,
                    );
                } else {
                    paths.push(arg.as_str());
                }
            }
            if paths.is_empty() {
                return Err("usage: loom add [--scan-workers N] [--] PATH [PATH ...]".into());
            }
            repo.add_paths_with_scan_options(
                &paths,
                WorktreeScanOptions {
                    workers: workers.unwrap_or(0),
                },
            )?;
            for path in paths {
                println!("Staged {path}");
            }
        }
        "unstage" => {
            if args.len() < 2 {
                return Err("usage: loom unstage PATH [PATH ...]".into());
            }
            repo.unstage_paths(&args[1..])?;
            for path in &args[1..] {
                println!("Unstaged {path}");
            }
        }
        "commit" => {
            let mut message = None;
            let mut author = None;
            let mut workers = None;
            let mut rest = args[1..].iter();
            while let Some(option) = rest.next() {
                let value = rest.next().ok_or("commit option requires a value")?;
                match option.as_str() {
                    "-m" if message.is_none() => message = Some(value.as_str()),
                    "--author" if author.is_none() => author = Some(value.clone()),
                    "--scan-workers" if workers.is_none() => {
                        workers = Some(value.parse::<usize>()?)
                    }
                    _ => return Err("unknown or duplicate commit option".into()),
                }
            }
            let message = message.ok_or("commit requires -m MESSAGE")?;
            let author = author.unwrap_or_else(|| {
                env::var("LOOM_AUTHOR")
                    .or_else(|_| env::var("USER"))
                    .or_else(|_| env::var("USERNAME"))
                    .unwrap_or_else(|_| "unknown".into())
            });
            let oid = repo.commit_with_scan_options(
                &author,
                message,
                WorktreeScanOptions {
                    workers: workers.unwrap_or(0),
                },
            )?;
            println!("Committed {oid}");
        }
        "log" => {
            let limit = args
                .get(1)
                .map(|s| s.parse::<usize>())
                .transpose()?
                .unwrap_or(20);
            for (oid, c) in repo.log(limit)? {
                println!(
                    "commit {oid}\nAuthor: {}\nDate: {}\n\n    {}\n",
                    c.author, c.timestamp_seconds, c.message
                );
            }
        }
        "branch" => match args.get(1).map(String::as_str) {
            Some("list") => {
                let current = repo.current_branch()?;
                for (name, tip) in repo.branches()? {
                    println!(
                        "{} {} {}",
                        if name == current { "*" } else { " " },
                        name,
                        tip.map(|x| x.to_string())
                            .unwrap_or_else(|| "(unborn)".into())
                    );
                }
            }
            Some("create") => {
                let name = args.get(2).ok_or("branch create requires NAME")?;
                repo.create_branch(name)?;
                println!("Created branch {name}");
            }
            _ => return Err("usage: loom branch list|create NAME".into()),
        },
        "switch" => {
            let name = args.get(1).ok_or("switch requires a branch name")?;
            repo.switch_branch(name)?;
            println!("Switched to branch {name}");
        }
        "merge" => {
            let name = args.get(1).ok_or("merge requires a source branch")?;
            let message = if let Some(pos) = args.iter().position(|x| x == "-m") {
                args.get(pos + 1).ok_or("missing -m value")?.clone()
            } else {
                format!("Merge branch {name}")
            };
            let author = env::var("LOOM_AUTHOR")
                .or_else(|_| env::var("USER"))
                .or_else(|_| env::var("USERNAME"))
                .unwrap_or_else(|_| "unknown".into());
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
                _ => {
                    return Err(
                        "usage: loom diff [--staged | --unstaged | FROM_BRANCH TO_BRANCH]".into(),
                    )
                }
            };
            for change in &changes {
                print_diff(change);
            }
        }
        "review" => {
            if args.len() != 1 {
                return Err("usage: loom review".into());
            }
            println!("=== Ready to commit (staged) ===");
            let staged = repo.diff_staged()?;
            if staged.is_empty() {
                println!("No staged changes");
            }
            for change in &staged {
                print_diff(change);
            }
            println!("=== Working changes (not staged) ===");
            let working = repo.diff_unstaged()?;
            if working.is_empty() {
                println!("No unstaged changes");
            }
            for change in &working {
                print_diff(change);
            }
            if !repo.staging_enabled()? {
                println!(
                    "Legacy commit-all mode: 'loom commit' currently includes all working changes."
                );
            }
        }
        "files" => {
            // Text mode quotes/escapes filenames, including embedded newlines.
            // --null uses NUL-delimited raw UTF-8 paths for tool pipelines.
            let null = args.last().is_some_and(|value| value == "--null");
            let positional = if null {
                &args[1..args.len() - 1]
            } else {
                &args[1..]
            };
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
        "tree" => match args.as_slice() {
            [_, revision] if revision.starts_with('@') => {
                let root = repo.native_tree_root(ObjectId::parse(&revision[1..])?)?;
                println!("{root}");
            }
            [_, action, root] if action == "ls" => {
                let hex = root
                    .strip_prefix('@')
                    .ok_or("tree ls requires a @TREE_ID")?;
                let entries = repo.native_tree_entries(ObjectId::parse(hex)?, None)?;
                for entry in entries {
                    let kind = match entry.kind {
                        loom_core::TreeEntryKind::File => "file",
                        loom_core::TreeEntryKind::Directory => "dir",
                    };
                    println!("{kind} {} {:?}", entry.object, entry.name);
                }
            }
            [_, action, root, directory] if action == "ls" => {
                let hex = root
                    .strip_prefix('@')
                    .ok_or("tree ls requires a @TREE_ID")?;
                let entries = repo.native_tree_entries(ObjectId::parse(hex)?, Some(directory))?;
                for entry in entries {
                    let kind = match entry.kind {
                        loom_core::TreeEntryKind::File => "file",
                        loom_core::TreeEntryKind::Directory => "dir",
                    };
                    println!("{kind} {} {:?}", entry.object, entry.name);
                }
            }
            [_, action, root, path] if action == "cat" => {
                let hex = root
                    .strip_prefix('@')
                    .ok_or("tree cat requires a @TREE_ID")?;
                let data = repo
                    .native_tree_file(ObjectId::parse(hex)?, path)?
                    .ok_or_else(|| format!("not a file in native tree: {path}"))?;
                use std::io::Write;
                std::io::stdout().lock().write_all(&data)?;
            }
            _ => {
                return Err(
                    "usage: loom tree @CHANGESET_ID | ls @TREE_ID [DIR] | cat @TREE_ID PATH".into(),
                )
            }
        },
        "cat" => {
            if args.len() != 3 {
                return Err("usage: loom cat BRANCH|@CHANGESET_ID PATH".into());
            }
            let data = if let Some(hex) = args[1].strip_prefix('@') {
                repo.read_revision_file(ObjectId::parse(hex)?, &args[2])?
            } else {
                repo.read_branch_file(&args[1], &args[2])?
            }
            .ok_or_else(|| format!("path not found at {}: {}", args[1], args[2]))?;
            use std::io::Write;
            std::io::stdout().lock().write_all(&data)?;
        }
        "fsck" => println!(
            "Verified {} native objects and branch references",
            repo.fsck()?
        ),
        "portable-manifest" => {
            let [_, raw_kind, raw_path] = args.as_slice() else {
                return Err("usage: loom portable-manifest regular|executable|symlink PATH".into());
            };
            let kind = match raw_kind.as_str() {
                "regular" => loom_core::PortableFileKind::Regular,
                "executable" => loom_core::PortableFileKind::Executable,
                "symlink" => loom_core::PortableFileKind::Symlink,
                _ => {
                    return Err(
                        "portable manifest kind must be regular, executable or symlink".into(),
                    )
                }
            };
            let path = Path::new(raw_path);
            let metadata = std::fs::symlink_metadata(path)?;
            let id = if kind == loom_core::PortableFileKind::Symlink {
                if !metadata.file_type().is_symlink() {
                    return Err("portable symlink manifest requires a symlink path".into());
                }
                let content = std::fs::read_link(path)?
                    .to_str()
                    .ok_or("portable symlink target must be UTF-8")?
                    .as_bytes()
                    .to_vec();
                repo.publish_portable_blob(kind, &content)?
            } else {
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err("portable file manifest requires a regular non-symlink file".into());
                }
                let mut input = std::fs::File::open(path)?;
                let opened = input.metadata()?;
                if !same_open_file(&metadata, &opened) {
                    return Err("portable manifest input changed before reading".into());
                }
                let id = repo.publish_portable_blob_from_reader(kind, opened.len(), &mut input)?;
                let opened_after = input.metadata()?;
                let after = std::fs::symlink_metadata(path)?;
                if !same_open_file(&opened, &opened_after) || !same_open_file(&opened_after, &after)
                {
                    return Err("portable manifest input changed while reading".into());
                }
                id
            };
            println!("{id}");
        }
        "portable-tree" => {
            if args.len() != 2 && args.len() != 4 {
                return Err("usage: loom portable-tree DIRECTORY [--workers N]".into());
            }
            let raw_path = &args[1];
            let workers = if args.len() == 4 {
                if args[2] != "--workers" {
                    return Err("usage: loom portable-tree DIRECTORY [--workers N]".into());
                }
                args[3]
                    .parse::<usize>()
                    .map_err(|_| "--workers requires a nonnegative integer")?
            } else {
                0
            };
            let id = publish_portable_directory(&repo, Path::new(raw_path), 0, workers)?;
            println!("{id}");
        }
        "portable-export" => {
            if args.len() != 3 && args.len() != 5 {
                return Err(
                    "usage: loom portable-export @TREE_ID DESTINATION [--workers N]".into(),
                );
            }
            let root = ObjectId::parse(
                args[1]
                    .strip_prefix('@')
                    .ok_or("portable-export requires an immutable @TREE_ID")?,
            )?;
            let workers = if args.len() == 5 {
                if args[3] != "--workers" {
                    return Err(
                        "usage: loom portable-export @TREE_ID DESTINATION [--workers N]".into(),
                    );
                }
                args[4]
                    .parse::<usize>()
                    .map_err(|_| "--workers requires a nonnegative integer")?
            } else {
                0
            };
            let stats = repo.export_portable_tree(root, &args[2], workers)?;
            println!(
                "Exported {} files, {} directories and {} bytes from {}",
                stats.files, stats.directories, stats.bytes, root
            );
        }
        "portable-verify" => {
            if args.len() != 3 && args.len() != 5 {
                return Err("usage: loom portable-verify @TREE_ID DIRECTORY [--workers N]".into());
            }
            let root = ObjectId::parse(
                args[1]
                    .strip_prefix('@')
                    .ok_or("portable-verify requires an immutable @TREE_ID")?,
            )?;
            let workers = if args.len() == 5 {
                if args[3] != "--workers" {
                    return Err(
                        "usage: loom portable-verify @TREE_ID DIRECTORY [--workers N]".into(),
                    );
                }
                args[4]
                    .parse::<usize>()
                    .map_err(|_| "--workers requires a nonnegative integer")?
            } else {
                0
            };
            let stats = repo.verify_portable_export(root, &args[2], workers)?;
            println!(
                "Verified {} files, {} directories and {} bytes from {}",
                stats.files, stats.directories, stats.bytes, root
            );
        }
        "portable-migrate-plan" => {
            if args.len() != 2 && args.len() != 4 {
                return Err("usage: loom portable-migrate-plan @CHANGESET_ID [--workers N]".into());
            }
            let revision = ObjectId::parse(
                args[1]
                    .strip_prefix('@')
                    .ok_or("portable-migrate-plan requires an immutable @CHANGESET_ID")?,
            )?;
            let workers = if args.len() == 4 {
                if args[2] != "--workers" {
                    return Err(
                        "usage: loom portable-migrate-plan @CHANGESET_ID [--workers N]".into(),
                    );
                }
                args[3]
                    .parse::<usize>()
                    .map_err(|_| "--workers requires a nonnegative integer")?
            } else {
                0
            };
            let plan = repo.plan_portable_migration(revision, workers)?;
            println!(
                "Portable root {}: {} files, {} directories, {} bytes, {} chunks, {} unique objects",
                plan.root,
                plan.files,
                plan.directories,
                plan.logical_bytes,
                plan.chunks,
                plan.objects
            );
        }
        _ => {
            usage();
            return Err(format!("unknown command: {cmd}").into());
        }
    }
    Ok(())
}
/// Run the explicit bridge before UTF-8 native argument parsing or discovery.
/// OS arguments preserve binary filenames and are passed directly, without a shell.
fn git_bridge(args: &[std::ffi::OsString]) -> Result<i32, Box<dyn std::error::Error>> {
    if args.get(1).map(|s| s.as_os_str()) != Some(std::ffi::OsStr::new("--")) || args.len() < 3 {
        return Err("usage: loom git -- GIT_ARGS...".into());
    }
    let status = process::Command::new("git")
        .args(&args[2..])
        .status()
        .map_err(|error| format!("cannot launch Git compatibility bridge: {error}"))?;
    if let Some(code) = status.code() {
        return Ok(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        Ok(128 + status.signal().unwrap_or(1))
    }
    #[cfg(not(unix))]
    Ok(1)
}

fn main() {
    let raw: Vec<_> = env::args_os().skip(1).collect();
    if raw.first().map(|s| s.as_os_str()) == Some(std::ffi::OsStr::new("git")) {
        match git_bridge(&raw) {
            Ok(code) => process::exit(code),
            Err(error) => {
                eprintln!("loom: {error}");
                process::exit(1);
            }
        }
    }
    if let Err(e) = run() {
        eprintln!("loom: {e}");
        process::exit(1);
    }
}
