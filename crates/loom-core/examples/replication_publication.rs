use loom_core::replicated_storage::ReplicatedObjectBackend;
use loom_core::storage_backend::ObjectBackend;
use loom_core::{FilesystemObjectBackend, Repository};
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

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
    let replica_paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
    if files == 0 || replica_paths.len() < 2 {
        return Err("positive file count and at least two replicas required".into());
    }

    std::fs::create_dir_all(&workspace)?;
    let mut replicas: Vec<Arc<dyn ObjectBackend>> = Vec::new();
    for path in &replica_paths {
        std::fs::create_dir_all(path)?;
        replicas.push(Arc::new(FilesystemObjectBackend::open(path)?));
    }
    let backend = Arc::new(ReplicatedObjectBackend::with_workers(replicas, workers)?);
    let repository = Repository::init_with_object_backend(&workspace, backend)?;
    for index in 0..files {
        let directory = workspace.join(format!("files/{:04x}", index % 64));
        std::fs::create_dir_all(&directory)?;
        let mut bytes = vec![0_u8; 256];
        bytes[..8].copy_from_slice(&(index as u64).to_le_bytes());
        std::fs::write(directory.join(format!("{index:08}.bin")), bytes)?;
    }

    let started = Instant::now();
    let revision = repository.commit("benchmark", "replicated publication")?;
    let elapsed = started.elapsed();
    let checked = repository.fsck()?;
    println!("{}\t{}\t{}", elapsed.as_nanos(), revision, checked);
    Ok(())
}
