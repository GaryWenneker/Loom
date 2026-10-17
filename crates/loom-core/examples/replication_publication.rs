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
    let mut copies = Vec::new();
    for path in &replica_paths {
        std::fs::create_dir_all(path)?;
        let replica = Arc::new(FilesystemObjectBackend::open(path)?);
        replicas.push(replica.clone());
        copies.push(replica);
    }
    let backend = Arc::new(ReplicatedObjectBackend::with_workers(replicas, workers)?);
    let repository = Repository::init_with_object_backend(&workspace, backend.clone())?;
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
    drop(repository);

    // Enumerate outside timing, then revalidate live bytes through every
    // replica. Includes stored orphan objects as well as the reachable graph.
    let objects = copies[0].list_ids()?;
    let verification_started = Instant::now();
    for id in &objects {
        if !backend.exists(*id)? {
            return Err("stored object missing from a replica".into());
        }
    }
    let verification_elapsed = verification_started.elapsed();

    let mut checked = Vec::with_capacity(copies.len());
    for copy in copies {
        let replica = Repository::discover_with_object_backend(&workspace, copy)?;
        if replica.head()?.as_ref() != Some(&revision) {
            return Err("replica did not expose the acknowledged revision".into());
        }
        checked.push(replica.fsck()?);
    }
    let checked = checked
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(",");
    println!(
        "{}\t{}\t{}\t{}\t{}",
        elapsed.as_nanos(),
        verification_elapsed.as_nanos(),
        objects.len(),
        revision,
        checked
    );
    Ok(())
}
