//! Synchronous verified replication across independent immutable object stores.
//! No distributed ref authority is implied by object replication.
use crate::format::{decode_object, object_id};
use crate::storage_backend::{ObjectBackend, StoreCapabilities};
use crate::{Error, ObjectId, Result};
use std::sync::Arc;

/// All replicas must acknowledge publication before success. A failed write
/// may leave immutable objects on a subset of replicas; retry is idempotent.
const MAX_REPLICA_WORKERS: usize = 8;

pub struct ReplicatedObjectBackend {
    replicas: Vec<Arc<dyn ObjectBackend>>,
    publication_workers: usize,
    verification_workers: usize,
}

impl ReplicatedObjectBackend {
    pub fn new(replicas: Vec<Arc<dyn ObjectBackend>>) -> Result<Self> {
        Self::with_workers(replicas, 0)
    }

    /// Configure bounded replica publication and live verification. Zero selects
    /// up to four publication workers and serial verification. Explicit requests
    /// apply to both phases, capped at eight and the replica count.
    pub fn with_workers(
        replicas: Vec<Arc<dyn ObjectBackend>>,
        requested_workers: usize,
    ) -> Result<Self> {
        if replicas.is_empty() {
            return Err(Error::Invalid(
                "replicated storage requires at least one backend".into(),
            ));
        }
        if replicas.iter().any(|r| {
            let c = r.capabilities();
            !c.durable_writes || !c.conditional_create
        }) {
            return Err(Error::Invalid(
                "replicated storage requires durable conditional-create backends".into(),
            ));
        }
        let automatic = requested_workers == 0;
        let requested_workers = if automatic { 4 } else { requested_workers };
        let publication_workers = requested_workers
            .min(MAX_REPLICA_WORKERS)
            .min(replicas.len())
            .max(1);
        Ok(Self {
            replicas,
            publication_workers,
            verification_workers: if automatic { 1 } else { publication_workers },
        })
    }

    pub fn replica_count(&self) -> usize {
        self.replicas.len()
    }
}

impl ObjectBackend for ReplicatedObjectBackend {
    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            durable_writes: true,
            conditional_create: true,
            streaming_reads: false,
            streaming_writes: false,
            list_objects: false,
        }
    }

    fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
        for replica in &self.replicas {
            if let Some(bytes) = verified_replica(replica.as_ref(), id)? {
                return Ok(bytes);
            }
        }
        Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("object {id} absent from all replicas"),
        )))
    }

    fn put_if_absent(&self, id: ObjectId, canonical_envelope: &[u8]) -> Result<()> {
        if object_id(canonical_envelope) != id {
            return Err(Error::Invalid(
                "object ID does not match canonical envelope".into(),
            ));
        }
        decode_object(canonical_envelope)?;
        // The outer object publisher intentionally sees this backend's
        // default single-write capability. Parallelism exists only across the
        // replicas for one object, preventing object-worker x replica-worker
        // multiplication while still overlapping independent durable I/O.
        crate::workers::parallel_map_min(
            &self.replicas,
            self.publication_workers,
            2,
            &|replica| {
                replica.put_if_absent(id, canonical_envelope)?;
                // A backend must not acknowledge bytes that cannot be verified.
                if replica.get(id)? != canonical_envelope {
                    return Err(Error::Invalid(format!(
                        "replica failed read-after-write verification for {id}"
                    )));
                }
                Ok(())
            },
        )?;
        Ok(())
    }

    fn exists(&self, id: ObjectId) -> Result<bool> {
        // Closure acknowledgment requires live, verified bytes on every
        // replica. Existence flags and retained read sessions cannot prove it.
        // Return only booleans from workers, releasing payloads immediately.
        let present = crate::workers::parallel_map_min(
            &self.replicas,
            self.verification_workers,
            2,
            &|replica| Ok(verified_replica(replica.as_ref(), id)?.is_some()),
        )?;
        Ok(present.into_iter().all(|value| value))
    }
}

fn verified_replica(replica: &dyn ObjectBackend, id: ObjectId) -> Result<Option<Vec<u8>>> {
    let bytes = match replica.get(id) {
        Ok(bytes) => bytes,
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if object_id(&bytes) != id {
        return Err(Error::Invalid(format!(
            "replica returned invalid hash for {id}"
        )));
    }
    decode_object(&bytes)?;
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{encode_object, BLOB};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Default)]
    struct FakeDurable {
        bytes: Mutex<HashMap<ObjectId, Vec<u8>>>,
        fail: AtomicBool,
    }
    impl ObjectBackend for FakeDurable {
        fn capabilities(&self) -> StoreCapabilities {
            StoreCapabilities {
                durable_writes: true,
                conditional_create: true,
                streaming_reads: false,
                streaming_writes: false,
                list_objects: false,
            }
        }
        fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
            self.bytes.lock().unwrap().get(&id).cloned().ok_or_else(|| {
                Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"))
            })
        }
        fn put_if_absent(&self, id: ObjectId, bytes: &[u8]) -> Result<()> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(Error::Io(std::io::Error::other("injected outage")));
            }
            self.bytes
                .lock()
                .unwrap()
                .entry(id)
                .or_insert_with(|| bytes.to_vec());
            Ok(())
        }
        fn exists(&self, id: ObjectId) -> Result<bool> {
            Ok(self.bytes.lock().unwrap().contains_key(&id))
        }
    }

    #[test]
    fn all_ack_and_retry_after_partial_failure() {
        let first = Arc::new(FakeDurable::default());
        let second = Arc::new(FakeDurable::default());
        second.fail.store(true, Ordering::SeqCst);
        let store = ReplicatedObjectBackend::new(vec![first.clone(), second.clone()]).unwrap();
        let bytes = encode_object(BLOB, b"replication").unwrap();
        let id = object_id(&bytes);
        assert!(store.put_if_absent(id, &bytes).is_err());
        assert!(!store.exists(id).unwrap());
        second.fail.store(false, Ordering::SeqCst);
        store.put_if_absent(id, &bytes).unwrap();
        assert!(store.exists(id).unwrap());
        assert_eq!(store.get(id).unwrap(), bytes);
    }

    #[test]
    fn exists_rejects_corrupt_secondary_replica() {
        let first = Arc::new(FakeDurable::default());
        let second = Arc::new(FakeDurable::default());
        let bytes = encode_object(BLOB, b"verified").unwrap();
        let id = object_id(&bytes);
        first.bytes.lock().unwrap().insert(id, bytes.clone());
        second.bytes.lock().unwrap().insert(id, b"corrupt".to_vec());
        for workers in [1, 4] {
            let store =
                ReplicatedObjectBackend::with_workers(vec![first.clone(), second.clone()], workers)
                    .unwrap();
            assert_eq!(store.get(id).unwrap(), bytes);
            assert!(store.exists(id).is_err());
        }
    }

    #[test]
    fn exists_checks_bytes_even_when_existence_flags_lie() {
        struct Missing;
        impl ObjectBackend for Missing {
            fn capabilities(&self) -> StoreCapabilities {
                FakeDurable::default().capabilities()
            }
            fn get(&self, _id: ObjectId) -> Result<Vec<u8>> {
                Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "missing",
                )))
            }
            fn put_if_absent(&self, _id: ObjectId, _bytes: &[u8]) -> Result<()> {
                Ok(())
            }
            fn exists(&self, _id: ObjectId) -> Result<bool> {
                Ok(true)
            }
        }
        let first = Arc::new(FakeDurable::default());
        let bytes = encode_object(BLOB, b"verified").unwrap();
        let id = object_id(&bytes);
        first.bytes.lock().unwrap().insert(id, bytes);
        for workers in [1, 4] {
            let store = ReplicatedObjectBackend::with_workers(
                vec![first.clone(), Arc::new(Missing)],
                workers,
            )
            .unwrap();
            assert!(!store.exists(id).unwrap());
        }
    }

    #[test]
    fn exists_rejects_hash_matching_noncanonical_envelope() {
        let replica = Arc::new(FakeDurable::default());
        let malformed = b"not a canonical Loom object".to_vec();
        let id = object_id(&malformed);
        replica.bytes.lock().unwrap().insert(id, malformed);
        let store = ReplicatedObjectBackend::new(vec![replica]).unwrap();
        assert!(store.exists(id).is_err());
    }

    #[test]
    fn live_verification_overlaps_and_never_uses_existence_flags() {
        use std::sync::Barrier;
        struct ObservedRead {
            bytes: Vec<u8>,
            barrier: Arc<Barrier>,
            active: Arc<AtomicUsize>,
            peak: Arc<AtomicUsize>,
            reads: Arc<AtomicUsize>,
        }
        impl ObjectBackend for ObservedRead {
            fn capabilities(&self) -> StoreCapabilities {
                FakeDurable::default().capabilities()
            }
            fn get(&self, _id: ObjectId) -> Result<Vec<u8>> {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(active, Ordering::SeqCst);
                self.barrier.wait();
                self.reads.fetch_add(1, Ordering::SeqCst);
                self.active.fetch_sub(1, Ordering::SeqCst);
                Ok(self.bytes.clone())
            }
            fn put_if_absent(&self, _id: ObjectId, _bytes: &[u8]) -> Result<()> {
                Ok(())
            }
            fn exists(&self, _id: ObjectId) -> Result<bool> {
                panic!("unverified existence flag used")
            }
        }
        let bytes = encode_object(BLOB, b"live verification").unwrap();
        let id = object_id(&bytes);
        let barrier = Arc::new(Barrier::new(4));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));
        let replicas = (0..4)
            .map(|_| {
                Arc::new(ObservedRead {
                    bytes: bytes.clone(),
                    barrier: barrier.clone(),
                    active: active.clone(),
                    peak: peak.clone(),
                    reads: reads.clone(),
                }) as Arc<dyn ObjectBackend>
            })
            .collect();
        let store = ReplicatedObjectBackend::with_workers(replicas, usize::MAX).unwrap();
        assert!(store.exists(id).unwrap());
        assert_eq!(peak.load(Ordering::SeqCst), 4);
        assert_eq!(reads.load(Ordering::SeqCst), 4);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        let many = (0..32)
            .map(|_| Arc::new(FakeDurable::default()) as Arc<dyn ObjectBackend>)
            .collect();
        assert_eq!(
            ReplicatedObjectBackend::with_workers(many, usize::MAX)
                .unwrap()
                .publication_workers,
            8
        );
        let default = ReplicatedObjectBackend::new(
            (0..8)
                .map(|_| Arc::new(FakeDurable::default()) as Arc<dyn ObjectBackend>)
                .collect(),
        )
        .unwrap();
        assert_eq!(default.publication_workers, 4);
        assert_eq!(default.verification_workers, 1);
    }

    #[test]
    fn verification_errors_and_panics_join_in_flight_reads() {
        use std::sync::mpsc;
        struct Failure(bool);
        impl ObjectBackend for Failure {
            fn capabilities(&self) -> StoreCapabilities {
                FakeDurable::default().capabilities()
            }
            fn get(&self, _id: ObjectId) -> Result<Vec<u8>> {
                assert!(!self.0, "injected replica read panic");
                Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected read error",
                )))
            }
            fn put_if_absent(&self, _id: ObjectId, _bytes: &[u8]) -> Result<()> {
                Ok(())
            }
            fn exists(&self, _id: ObjectId) -> Result<bool> {
                Ok(true)
            }
        }
        struct Blocked {
            bytes: Vec<u8>,
            started: mpsc::Sender<()>,
            release: Mutex<mpsc::Receiver<()>>,
            active: Arc<AtomicBool>,
        }
        impl ObjectBackend for Blocked {
            fn capabilities(&self) -> StoreCapabilities {
                FakeDurable::default().capabilities()
            }
            fn get(&self, _id: ObjectId) -> Result<Vec<u8>> {
                self.active.store(true, Ordering::SeqCst);
                self.started.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
                self.active.store(false, Ordering::SeqCst);
                Ok(self.bytes.clone())
            }
            fn put_if_absent(&self, _id: ObjectId, _bytes: &[u8]) -> Result<()> {
                Ok(())
            }
            fn exists(&self, _id: ObjectId) -> Result<bool> {
                Ok(true)
            }
        }
        for panic in [false, true] {
            let bytes = encode_object(BLOB, b"join live read").unwrap();
            let id = object_id(&bytes);
            let (started, receive_started) = mpsc::channel();
            let (release, receive_release) = mpsc::channel();
            let active = Arc::new(AtomicBool::new(false));
            let backend = ReplicatedObjectBackend::with_workers(
                vec![
                    Arc::new(Failure(panic)),
                    Arc::new(Blocked {
                        bytes,
                        started,
                        release: Mutex::new(receive_release),
                        active: active.clone(),
                    }),
                ],
                2,
            )
            .unwrap();
            let coordinator = std::thread::spawn(move || backend.exists(id));
            receive_started
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            assert!(active.load(Ordering::SeqCst));
            assert!(!coordinator.is_finished());
            release.send(()).unwrap();
            assert!(coordinator.join().unwrap().is_err());
            assert!(!active.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn corrupt_replica_blocks_ref_publication_and_preserves_staging_for_retry() {
        let first = Arc::new(FakeDurable::default());
        let second = Arc::new(FakeDurable::default());
        let backend =
            Arc::new(ReplicatedObjectBackend::new(vec![first.clone(), second.clone()]).unwrap());
        let temp = tempfile::tempdir().unwrap();
        let repo = crate::Repository::init_with_object_backend(temp.path(), backend).unwrap();
        std::fs::write(temp.path().join("file"), b"original").unwrap();
        let before = repo.commit("test", "base").unwrap();
        std::fs::write(temp.path().join("file"), b"selected update").unwrap();
        repo.add("file").unwrap();
        let staged_before = repo.diff_staged().unwrap();
        let bytes = encode_object(BLOB, b"original").unwrap();
        let id = object_id(&bytes);
        second.bytes.lock().unwrap().insert(id, b"corrupt".to_vec());
        assert!(repo.commit("test", "must reject corrupt closure").is_err());
        assert_eq!(repo.head().unwrap(), Some(before));
        assert_eq!(repo.diff_staged().unwrap(), staged_before);
        second.bytes.lock().unwrap().insert(id, bytes);
        let after = repo.commit("test", "retry verified closure").unwrap();
        assert_ne!(after, before);
        assert_eq!(
            repo.read_branch_file("main", "file").unwrap().unwrap(),
            b"selected update"
        );
        drop(repo);
        // Reopen validates the complete acknowledged history on each copy.
        // This fake provider intentionally has no full-enumeration fsck API.
        for copy in [first, second] {
            let reopened =
                crate::Repository::discover_with_object_backend(temp.path(), copy).unwrap();
            assert_eq!(reopened.head().unwrap(), Some(after));
            assert_eq!(
                reopened.read_branch_file("main", "file").unwrap().unwrap(),
                b"selected update"
            );
        }
    }

    #[test]
    fn rejects_non_durable_replica() {
        struct Volatile;
        impl ObjectBackend for Volatile {
            fn capabilities(&self) -> StoreCapabilities {
                StoreCapabilities {
                    durable_writes: false,
                    conditional_create: true,
                    streaming_reads: false,
                    streaming_writes: false,
                    list_objects: false,
                }
            }
            fn get(&self, _: ObjectId) -> Result<Vec<u8>> {
                unreachable!()
            }
            fn put_if_absent(&self, _: ObjectId, _: &[u8]) -> Result<()> {
                unreachable!()
            }
            fn exists(&self, _: ObjectId) -> Result<bool> {
                unreachable!()
            }
        }
        assert!(ReplicatedObjectBackend::new(vec![Arc::new(Volatile)]).is_err());
    }
    #[test]
    fn replica_publications_overlap_with_bounded_workers() {
        use std::sync::Barrier;
        struct ObservedDurable {
            inner: FakeDurable,
            active: Arc<AtomicUsize>,
            peak: Arc<AtomicUsize>,
            barrier: Arc<Barrier>,
        }
        impl ObjectBackend for ObservedDurable {
            fn capabilities(&self) -> StoreCapabilities {
                self.inner.capabilities()
            }
            fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
                self.inner.get(id)
            }
            fn put_if_absent(&self, id: ObjectId, bytes: &[u8]) -> Result<()> {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(active, Ordering::SeqCst);
                self.barrier.wait();
                let result = self.inner.put_if_absent(id, bytes);
                self.active.fetch_sub(1, Ordering::SeqCst);
                result
            }
            fn exists(&self, id: ObjectId) -> Result<bool> {
                self.inner.exists(id)
            }
        }

        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(4));
        let replicas: Vec<Arc<dyn ObjectBackend>> = (0..4)
            .map(|_| {
                Arc::new(ObservedDurable {
                    inner: FakeDurable::default(),
                    active: active.clone(),
                    peak: peak.clone(),
                    barrier: barrier.clone(),
                }) as Arc<dyn ObjectBackend>
            })
            .collect();
        let store = ReplicatedObjectBackend::with_workers(replicas, usize::MAX).unwrap();
        assert_eq!(store.publication_workers, 4);
        let bytes = encode_object(BLOB, b"parallel replication").unwrap();
        let id = object_id(&bytes);
        store.put_if_absent(id, &bytes).unwrap();
        assert_eq!(peak.load(Ordering::SeqCst), 4);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!(store.exists(id).unwrap());
    }
}
