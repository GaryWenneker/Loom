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

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let workspace = PathBuf::from(args.next().ok_or("missing workspace")?);
    let workers: usize = args
        .next()
        .ok_or("missing worker count")?
        .to_string_lossy()
        .parse()?;
    let files: usize = args
        .next()
        .ok_or("missing file count")?
        .to_string_lossy()
        .parse()?;
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

    let started = Instant::now();
    let revision =
        repository.merge_branch_with_workers("feature", "benchmark", "parallel merge", workers)?;
    let elapsed = started.elapsed();
    let root = repository.native_tree_root(revision)?;

    for index in 0..files {
        let path = workspace.join(format!("files/{:04x}/{index:08}.txt", index % 64));
        if std::fs::read(path)? != contents(index, "merged") {
            return Err(format!("merged bytes differ at file {index}").into());
        }
    }
    if repository.status()?.len() != 0 {
        return Err("merge left a dirty worktree".into());
    }
    let checked = repository.fsck()?;
    println!("{}\t{}\t{}\t{}", elapsed.as_nanos(), revision, root, checked);
    Ok(())
}
