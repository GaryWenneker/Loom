//! Full-content scans with bounded payload windows, independent of repository size.
use crate::format::{object_header, object_id, validate_path, BLOB, MAX_OBJECT_SIZE};
use crate::{Change, ChangeKind, Error, ObjectId, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

const WINDOW_FILES: usize = 1024;
const WINDOW_BYTES: usize = 8 * 1024 * 1024;
const MAX_WORKERS: usize = 8;

/// Limits for full-content status and legacy commit-all scans.
/// Zero selects up to four workers. Other requests are capped at eight and
/// available CPUs. Small windows and files larger than 8 MiB run serially.
#[derive(Clone, Copy, Debug, Default)]
pub struct WorktreeScanOptions {
    pub workers: usize,
}

struct Work {
    path: String,
    absolute: PathBuf,
    length: usize,
    prior: Option<ObjectId>,
}

/// Validate every selected ancestor without reading unrelated file contents.
/// Like other v1 traversal, this is not race-free against parent substitution.
pub(crate) fn inspect_selected(root: &Path, path: &str) -> Result<Option<(PathBuf, fs::Metadata)>> {
    validate_path(path)?;
    let mut absolute = root.to_path_buf();
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        absolute.push(part);
        let metadata = match fs::symlink_metadata(&absolute) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() {
            return Err(Error::Invalid(format!("symlink in selected path: {path}")));
        }
        if parts.peek().is_some() {
            if !metadata.is_dir() {
                return Ok(None);
            }
        } else {
            if !metadata.is_file() && !metadata.is_dir() {
                return Err(Error::Invalid(format!(
                    "unsupported selected file type: {path}"
                )));
            }
            if metadata.is_file() && metadata.len() > MAX_OBJECT_SIZE as u64 {
                return Err(Error::Invalid(format!("file exceeds 64 MiB: {path}")));
            }
            return Ok(Some((absolute, metadata)));
        }
    }
    Ok(None)
}

pub(crate) fn selected_changes(
    root: &Path,
    selected: &BTreeMap<String, Option<ObjectId>>,
    options: WorktreeScanOptions,
    mut publish: impl FnMut(ObjectId, Vec<u8>) -> Result<()>,
) -> Result<Vec<Change>> {
    let mut pending = Vec::new();
    let mut window_bytes = 0;
    let mut workers = None;
    let mut changes = Vec::new();
    for (path, prior) in selected {
        let input = inspect_selected(root, path)?;
        if let Some((absolute, metadata)) = input.filter(|(_, metadata)| metadata.is_file()) {
            let length = metadata.len() as usize;
            if pending.len() == WINDOW_FILES || window_bytes + length > WINDOW_BYTES {
                flush(
                    &mut pending,
                    options,
                    &mut workers,
                    &mut changes,
                    &mut publish,
                )?;
                window_bytes = 0;
            }
            pending.push(Work {
                path: path.clone(),
                absolute,
                length,
                prior: *prior,
            });
            window_bytes += length;
            if length > WINDOW_BYTES {
                flush(
                    &mut pending,
                    options,
                    &mut workers,
                    &mut changes,
                    &mut publish,
                )?;
                window_bytes = 0;
            }
        } else if prior.is_some() {
            changes.push(Change {
                kind: ChangeKind::Delete,
                path: path.clone(),
                blob: None,
            });
        }
    }
    flush(
        &mut pending,
        options,
        &mut workers,
        &mut changes,
        &mut publish,
    )?;
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

/// Read at most the discovered length plus one byte. Growing/shrinking files
/// fail instead of escaping the payload window. This is not an atomic snapshot
/// against external writers, or handle-relative traversal of parent directories.
fn read(work: &Work) -> Result<(ObjectId, Vec<u8>)> {
    #[cfg(not(unix))]
    {
        let metadata = fs::symlink_metadata(&work.absolute)?;
        if !metadata.is_file() || metadata.len() != work.length as u64 {
            return Err(Error::Invalid(format!(
                "file changed during scan: {}",
                work.path
            )));
        }
    }
    #[cfg(unix)]
    let file = {
        use rustix::fs::{open, Mode, OFlags};
        File::from(
            open(
                &work.absolute,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        )
    };
    #[cfg(not(unix))]
    let file = File::open(&work.absolute)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != work.length as u64 {
        return Err(Error::Invalid(format!(
            "file changed during scan: {}",
            work.path
        )));
    }
    let header = object_header(BLOB, work.length)?;
    let mut bytes = Vec::with_capacity(work.length + header.len() + 1);
    bytes.extend_from_slice(&header);
    file.take(work.length as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() != work.length + header.len() {
        return Err(Error::Invalid(format!(
            "file changed during scan: {}",
            work.path
        )));
    }
    Ok((object_id(&bytes), bytes))
}

pub(crate) fn worker_limit(requested: usize, available: usize) -> usize {
    let requested = if requested == 0 { 4 } else { requested };
    requested.min(MAX_WORKERS).min(available).max(1)
}

fn flush(
    pending: &mut Vec<Work>,
    options: WorktreeScanOptions,
    workers: &mut Option<usize>,
    changes: &mut Vec<Change>,
    publish: &mut impl FnMut(ObjectId, Vec<u8>) -> Result<()>,
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let count = if pending.len() < 32 || pending[0].length > WINDOW_BYTES {
        1
    } else {
        *workers.get_or_insert_with(|| {
            worker_limit(
                options.workers,
                std::thread::available_parallelism().map_or(1, usize::from),
            )
        })
    };
    let results = crate::workers::parallel_map(pending, count, &|work| {
        let (id, bytes) = read(work)?;
        Ok((id, (work.prior != Some(id)).then_some(bytes)))
    })?;
    for (work, (id, encoded)) in pending.drain(..).zip(results) {
        if let Some(encoded) = encoded {
            publish(id, encoded)?;
            changes.push(Change {
                kind: if work.prior.is_some() {
                    ChangeKind::Modify
                } else {
                    ChangeKind::Add
                },
                path: work.path,
                blob: Some(id),
            });
        }
    }
    Ok(())
}

pub(crate) fn changes(
    root: &Path,
    old: &BTreeMap<String, ObjectId>,
    options: WorktreeScanOptions,
    mut publish: impl FnMut(ObjectId, Vec<u8>) -> Result<()>,
) -> Result<Vec<Change>> {
    let ignores = crate::ignore::IgnoreRules::load(root)?;
    let mut missing: BTreeSet<_> = old.keys().map(String::as_str).collect();
    let mut stack = vec![root.to_path_buf()];
    let mut pending = Vec::new();
    let mut window_bytes = 0;
    let mut workers = None;
    let mut changes = Vec::new();
    while let Some(dir) = stack.pop() {
        for item in fs::read_dir(dir)? {
            let item = item?;
            if item.file_name() == ".loom" || item.file_name() == ".git" {
                continue;
            }
            let absolute = item.path();
            let relative = absolute
                .strip_prefix(root)
                .map_err(|_| Error::Invalid("path escaped worktree".into()))?;
            let relative = relative
                .to_str()
                .ok_or_else(|| Error::Invalid("non-UTF-8 path not supported in v1".into()))?;
            let path = if cfg!(windows) {
                relative.replace('\\', "/")
            } else {
                relative.to_string()
            };
            validate_path(&path)?;
            let kind = item.file_type()?;
            if kind.is_dir() {
                stack.push(absolute);
                continue;
            }
            let prior = old.get(&path).copied();
            if prior.is_none() && ignores.ignores(&path) {
                continue;
            }
            if !kind.is_file() {
                return Err(Error::Invalid(format!(
                    "unsupported file type in scan: {path}"
                )));
            }
            let length = item.metadata()?.len();
            if length > MAX_OBJECT_SIZE as u64 {
                return Err(Error::Invalid(format!("file exceeds 64 MiB: {path}")));
            }
            let length = length as usize;
            if pending.len() == WINDOW_FILES || window_bytes + length > WINDOW_BYTES {
                flush(
                    &mut pending,
                    options,
                    &mut workers,
                    &mut changes,
                    &mut publish,
                )?;
                window_bytes = 0;
            }
            missing.remove(path.as_str());
            pending.push(Work {
                path,
                absolute,
                length,
                prior,
            });
            window_bytes += length;
            if length > WINDOW_BYTES {
                flush(
                    &mut pending,
                    options,
                    &mut workers,
                    &mut changes,
                    &mut publish,
                )?;
                window_bytes = 0;
            }
        }
    }
    flush(
        &mut pending,
        options,
        &mut workers,
        &mut changes,
        &mut publish,
    )?;
    changes.extend(missing.into_iter().map(|path| Change {
        kind: ChangeKind::Delete,
        path: path.to_string(),
        blob: None,
    }));
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::encode_object;

    #[test]
    fn worker_requests_never_escape_cpu_or_hard_limits() {
        assert_eq!(worker_limit(0, 64), 4);
        assert_eq!(worker_limit(usize::MAX, 64), 8);
        assert_eq!(worker_limit(usize::MAX, 2), 2);
        assert_eq!(worker_limit(1, 64), 1);
        assert_eq!(worker_limit(0, 1), 1);
    }

    #[test]
    fn discovery_length_drift_rejects_growth_and_shrinkage() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        let work = Work {
            path: "file".into(),
            absolute: path.clone(),
            length: 4,
            prior: None,
        };
        for bytes in [b"short".as_slice(), b"x".as_slice()] {
            fs::write(&path, bytes).unwrap();
            assert!(read(&work).is_err());
        }
        fs::write(&path, b"\0\xff\n\r").unwrap();
        let (id, bytes) = read(&work).unwrap();
        let expected = encode_object(BLOB, b"\0\xff\n\r").unwrap();
        assert_eq!(bytes, expected);
        assert_eq!(id, object_id(&expected));
    }

    #[test]
    fn failed_read_window_publishes_nothing_and_leaves_no_workers() {
        let temp = tempfile::tempdir().unwrap();
        let work: Vec<_> = (0..64)
            .map(|n| {
                let absolute = temp.path().join(n.to_string());
                if n != 32 {
                    fs::write(&absolute, b"x").unwrap();
                }
                Work {
                    path: n.to_string(),
                    absolute,
                    length: 1,
                    prior: None,
                }
            })
            .collect();
        let mut changes = Vec::new();
        let mut pending = work;
        let mut publishes = 0;
        assert!(flush(
            &mut pending,
            WorktreeScanOptions { workers: 4 },
            &mut Some(4),
            &mut changes,
            &mut |_, _| {
                publishes += 1;
                Ok(())
            }
        )
        .is_err());
        assert_eq!(publishes, 0);
        assert!(changes.is_empty());
    }

    #[test]
    fn unchanged_payloads_are_discarded_before_publication() {
        let temp = tempfile::tempdir().unwrap();
        let id = object_id(&encode_object(BLOB, b"same").unwrap());
        let old: BTreeMap<_, _> = (0..1200)
            .map(|n| {
                let name = format!("{n:04}");
                fs::write(temp.path().join(&name), b"same").unwrap();
                (name, id)
            })
            .collect();
        let changes = changes(
            temp.path(),
            &old,
            WorktreeScanOptions { workers: 8 },
            |_, _| panic!("unchanged content must not be published"),
        )
        .unwrap();
        assert!(changes.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn replaced_leaf_symlink_is_not_read() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        fs::write(&target, b"secret").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(read(&Work {
            path: "link".into(),
            absolute: link,
            length: 6,
            prior: None
        })
        .is_err());
    }
}
