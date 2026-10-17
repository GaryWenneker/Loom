use loom_core::Repository;
use std::error::Error;
use std::path::PathBuf;
use std::time::Instant;

fn contents(index: usize, side: &str) -> Vec<u8> {
    match side {
        "base" => format!("base-left-{index}\nshared-{index}\nbase-right-{index}\n").into_bytes(),
        "ours" => format!("ours-left-{index}\nshared-{index}\nbase-right-{index}\n").into_bytes(),
        "theirs" => {
            format!("base-left-{index}\nshared-{index}\ntheirs-right-{index}\n").into_bytes()
        }
        "merged" => {
            format!("ours-left-{index}\nshared-{index}\ntheirs-right-{index}\n").into_bytes()
        }
        _ => unreachable!(),
    }
}

fn write_files(workspace: &PathBuf, files: usize, side: &str) -> Result<(), Box<dyn Error>> {
    for index in 0..files {
        let directory = workspace.join(format!("files/{:04x}", index % 64));
        std::fs::create_dir_all(&directory)?;
        std::fs::write(directory.join(format!("{index:08}.txt")), contents(index, side))?;
    }
    Ok(())
}

fn parse_usize(value: Option<std::ffi::OsString>, name: &str) -> Result<usize, Box<dyn Error>> {
    Ok(value
        .ok_or_else(|| format!("missing {name}"))?
        .to_string_lossy()
        .parse()?)
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let operation = args
        .next()
        .ok_or("missing operation")?
        .to_string_lossy()
        .into_owned();
    let workspace = PathBuf::from(args.next().ok_or("missing workspace")?);
    match operation.as_str() {
        "prepare" => {
            let files = parse_usize(args.next(), "file count")?;
            if files == 0 {
                return Err("positive file count required".into());
            }
            std::fs::create_dir_all(&workspace)?;
            let repository = Repository::init(&workspace)?;
            write_files(&workspace, files, "base")?;
            repository.commit("benchmark", "base")?;
            repository.create_branch("feature")?;
            repository.switch_branch("feature")?;
            write_files(&workspace, files, "theirs")?;
            repository.commit("benchmark", "feature")?;
            repository.switch_branch("main")?;
            write_files(&workspace, files, "ours")?;
            repository.commit("benchmark", "main")?;
            if !repository.status()?.is_empty() {
                return Err("prepared worktree is dirty".into());
            }
        }
        "merge" => {
            let workers = parse_usize(args.next(), "worker count")?;
            let repository = Repository::discover(&workspace)?;
            let started = Instant::now();
            let revision = repository.merge_branch_with_workers(
                "feature",
                "benchmark",
                "parallel merge",
                workers,
            )?;
            let elapsed = started.elapsed();
            let root = repository.native_tree_root(revision)?;
            println!("{}\t{}\t{}", elapsed.as_nanos(), revision, root);
        }
        "verify" => {
            let files = parse_usize(args.next(), "file count")?;
            let expected_root = args
                .next()
                .ok_or("missing expected root")?
                .to_string_lossy()
                .into_owned();
            let repository = Repository::discover(&workspace)?;
            let revision = repository.head()?.ok_or("missing merge revision")?;
            if repository.native_tree_root(revision)?.to_string() != expected_root {
                return Err("merge root differs from expected canonical root".into());
            }
            for index in 0..files {
                let path = workspace.join(format!("files/{:04x}/{index:08}.txt", index % 64));
                if std::fs::read(path)? != contents(index, "merged") {
                    return Err(format!("merged bytes differ at file {index}").into());
                }
            }
            if !repository.status()?.is_empty() {
                return Err("merge left a dirty worktree".into());
            }
            println!("{}", repository.fsck()?);
        }
        _ => return Err(format!("unknown operation: {operation}").into()),
    }
    Ok(())
}
