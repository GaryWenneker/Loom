//! Immutable canonical objects in a durable SQLite database.
//! Object transactions finish before the repository publishes local branch refs.
use crate::format::{decode_object, object_id, MAX_OBJECT_SIZE};
use crate::storage_backend::{
    ObjectBackend, ObjectReadSession, ObjectWriteSession, StoreCapabilities,
};
use crate::{Error, ObjectId, Result};
use rusqlite::{
    params, Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

const READ_OBJECT_SQL: &str = "SELECT CASE WHEN length(envelope)<=?2 THEN envelope ELSE NULL END
     FROM loom_storage_objects WHERE oid=?1";
const LOCK_RETRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const LOCK_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(10);
const WRITE_GATE_STRIPES: usize = 64;
const SCALAR_READER_LIMIT: usize = 8;

fn write_gates() -> &'static [Mutex<()>; WRITE_GATE_STRIPES] {
    static GATES: std::sync::OnceLock<[Mutex<()>; WRITE_GATE_STRIPES]> = std::sync::OnceLock::new();
    GATES.get_or_init(|| std::array::from_fn(|_| Mutex::new(())))
}

fn write_gate(path: &Path) -> Result<std::sync::MutexGuard<'static, ()>> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hash);
    let stripe = hash.finish() as usize % WRITE_GATE_STRIPES;
    write_gates()[stripe]
        .lock()
        .map_err(|_| Error::Invalid("SQLite write gate poisoned".into()))
}

fn is_retryable_lock_code(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked | ErrorCode::FileLockingProtocolFailed
    )
}

fn is_retryable_lock_error(error: &rusqlite::Error) -> bool {
    match error {
        rusqlite::Error::SqliteFailure(error, _) => is_retryable_lock_code(error.code),
        _ => false,
    }
}

fn begin_immediate_with_retry(connection: &Connection) -> rusqlite::Result<Transaction<'_>> {
    // SQLite's busy handler does not cover SQLITE_PROTOCOL. In WAL mode that
    // result can be transient when many processes race to begin a write. Only
    // retry BEGIN IMMEDIATE: after it succeeds, every statement and COMMIT is
    // still reported directly so an ambiguous durable outcome is never replayed.
    let mut retry_deadline = None;
    loop {
        match Transaction::new_unchecked(connection, TransactionBehavior::Immediate) {
            Ok(transaction) => return Ok(transaction),
            Err(error) if is_retryable_lock_error(&error) => {
                let deadline = retry_deadline
                    .get_or_insert_with(|| std::time::Instant::now() + LOCK_RETRY_TIMEOUT);
                if std::time::Instant::now() >= *deadline {
                    return Err(error);
                }
                std::thread::sleep(LOCK_RETRY_DELAY);
            }
            Err(error) => return Err(error),
        }
    }
}

pub struct SqliteObjectBackend {
    path: PathBuf,
    identity: DatabaseIdentity,
    scalar_readers: ScalarReaderPool,
}

struct ScalarReaderPool {
    state: Mutex<ScalarReaderState>,
    ready: Condvar,
}

#[derive(Default)]
struct ScalarReaderState {
    idle: Vec<Connection>,
    opened: usize,
}

impl ScalarReaderPool {
    fn new() -> Self {
        Self {
            state: Mutex::new(ScalarReaderState::default()),
            ready: Condvar::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DatabaseIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    creation_time: u64,
}

impl DatabaseIdentity {
    fn read(path: &Path) -> Result<Self> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(Error::Invalid(
                "SQLite object database must be a regular file".into(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            Ok(Self {
                creation_time: metadata.creation_time(),
            })
        }
    }
}

struct SqliteWriteSession {
    path: PathBuf,
    identity: DatabaseIdentity,
    connection: Connection,
}

impl SqliteWriteSession {
    fn check_identity(&self) -> Result<()> {
        if DatabaseIdentity::read(&self.path)? != self.identity {
            return Err(Error::Invalid(
                "SQLite object database changed during write session".into(),
            ));
        }
        Ok(())
    }
}

impl ObjectWriteSession for SqliteWriteSession {
    fn put_batch_if_absent(&mut self, objects: &[(ObjectId, &[u8])]) -> Result<bool> {
        let _gate = write_gate(&self.path)?;
        self.check_identity()?;
        SqliteObjectBackend::write_batch(&mut self.connection, objects)?;
        self.check_identity()?;
        Ok(true)
    }
}

fn checked_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() || path == Path::new(":memory:") {
        return Err(Error::Invalid(
            "SQLite object storage requires a persistent file".into(),
        ));
    }
    let name = path
        .file_name()
        .ok_or_else(|| Error::Invalid("database filename required".into()))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let absolute = std::fs::canonicalize(parent)?.join(name);
    match std::fs::symlink_metadata(&absolute) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err(Error::Invalid(
                "SQLite object database must be a regular file".into(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
        _ => {}
    }
    Ok(absolute)
}

impl SqliteObjectBackend {
    /// Open an existing provider without creating a database or schema.
    pub fn open_existing(path: impl AsRef<Path>) -> Result<Self> {
        let path = checked_path(path.as_ref())?;
        let identity = DatabaseIdentity::read(&path)?;
        let backend = Self {
            path,
            identity,
            scalar_readers: ScalarReaderPool::new(),
        };
        let conn = backend.read_connection()?;
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(Error::Invalid(
                "SQLite object backend requires WAL journal mode".into(),
            ));
        }
        conn.prepare("SELECT oid,envelope FROM loom_storage_objects LIMIT 0")?;
        backend.check_identity()?;
        Ok(backend)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = checked_path(path.as_ref())?;
        let _gate = write_gate(&path)?;
        let conn = Connection::open(&path)?;
        conn.busy_timeout(LOCK_RETRY_TIMEOUT)?;
        // Concurrent initializers can need a journal-mode lock upgrade, which
        // SQLite may reject without invoking its busy handler. Retry the
        // idempotent bootstrap within one bounded deadline.
        let mut retry_deadline = None;
        let mode = loop {
            let initialized = (|| -> rusqlite::Result<String> {
                let current: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
                let mode = if current.eq_ignore_ascii_case("wal") {
                    current
                } else {
                    conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?
                };
                conn.execute_batch(
                    "PRAGMA synchronous=FULL;
                     CREATE TABLE IF NOT EXISTS loom_storage_objects (
                         oid TEXT PRIMARY KEY NOT NULL CHECK(length(oid)=64),
                         envelope BLOB NOT NULL
                     );",
                )?;
                Ok(mode)
            })();
            match initialized {
                Ok(mode) => break mode,
                Err(error) if is_retryable_lock_error(&error) => {
                    let deadline = retry_deadline
                        .get_or_insert_with(|| std::time::Instant::now() + LOCK_RETRY_TIMEOUT);
                    if std::time::Instant::now() >= *deadline {
                        return Err(error.into());
                    }
                    std::thread::sleep(LOCK_RETRY_DELAY);
                }
                Err(error) => return Err(error.into()),
            }
        };
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(Error::Invalid(
                "SQLite object backend requires WAL journal mode".into(),
            ));
        }
        let identity = DatabaseIdentity::read(&path)?;
        Ok(Self {
            path,
            identity,
            scalar_readers: ScalarReaderPool::new(),
        })
    }

    fn check_identity(&self) -> Result<()> {
        if DatabaseIdentity::read(&self.path)? != self.identity {
            return Err(Error::Invalid(
                "SQLite object database changed after backend open".into(),
            ));
        }
        Ok(())
    }

    fn scalar_read<T>(&self, read: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.check_identity()?;
        let mut state = self
            .scalar_readers
            .state
            .lock()
            .map_err(|_| Error::Invalid("SQLite scalar reader pool poisoned".into()))?;
        let connection = loop {
            if let Some(connection) = state.idle.pop() {
                break connection;
            }
            if state.opened < SCALAR_READER_LIMIT {
                state.opened += 1;
                drop(state);
                match self.read_connection() {
                    Ok(connection) => break connection,
                    Err(error) => {
                        let mut state = self.scalar_readers.state.lock().map_err(|_| {
                            Error::Invalid("SQLite scalar reader pool poisoned".into())
                        })?;
                        state.opened -= 1;
                        self.scalar_readers.ready.notify_one();
                        return Err(error);
                    }
                }
            }
            state = self
                .scalar_readers
                .ready
                .wait(state)
                .map_err(|_| Error::Invalid("SQLite scalar reader pool poisoned".into()))?;
        };
        if let Err(error) = self.check_identity() {
            self.return_scalar_reader(connection)?;
            return Err(error);
        }
        let result = read(&connection);
        let identity_result = self.check_identity();
        self.return_scalar_reader(connection)?;
        identity_result?;
        result
    }

    fn return_scalar_reader(&self, connection: Connection) -> Result<()> {
        let mut state = self
            .scalar_readers
            .state
            .lock()
            .map_err(|_| Error::Invalid("SQLite scalar reader pool poisoned".into()))?;
        state.idle.push(connection);
        self.scalar_readers.ready.notify_one();
        Ok(())
    }

    fn connection(&self) -> Result<Connection> {
        checked_path(&self.path)?;
        // Never recreate a database lost/replaced after opening the provider.
        let conn = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(LOCK_RETRY_TIMEOUT)?;
        conn.execute_batch("PRAGMA synchronous=FULL;")?;
        Ok(conn)
    }

    fn read_connection(&self) -> Result<Connection> {
        checked_path(&self.path)?;
        let conn = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(LOCK_RETRY_TIMEOUT)?;
        conn.execute_batch("PRAGMA query_only=ON;")?;
        Ok(conn)
    }

    fn read(conn: &Connection, id: ObjectId) -> Result<Vec<u8>> {
        let mut query = conn.prepare_cached(READ_OBJECT_SQL)?;
        Self::read_bounded(&mut query, id, MAX_OBJECT_SIZE + 15)
    }

    fn read_bounded(
        query: &mut rusqlite::Statement<'_>,
        id: ObjectId,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        let row: Option<Option<Vec<u8>>> = query
            .query_row(
                params![id.to_string(), max_bytes.min(MAX_OBJECT_SIZE + 15) as i64],
                |r| r.get(0),
            )
            .optional()?;
        let bytes = match row {
            None => {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("missing object {id}"),
                )))
            }
            Some(None) => {
                return Err(Error::Invalid(format!(
                    "SQLite object exceeds read byte limit: {id}"
                )))
            }
            Some(Some(bytes)) => bytes,
        };
        if object_id(&bytes) != id {
            return Err(Error::Invalid(format!("SQLite object hash mismatch: {id}")));
        }
        decode_object(&bytes)?;
        Ok(bytes)
    }

    fn write_batch(conn: &mut Connection, objects: &[(ObjectId, &[u8])]) -> Result<()> {
        // Reject the complete window before BEGIN IMMEDIATE so malformed input
        // cannot flush an earlier prefix. ObjectPublisher already bounds normal
        // calls to 128 objects / 8 MiB, while direct callers retain format limits.
        let mut unique = std::collections::BTreeMap::new();
        for (id, bytes) in objects {
            if object_id(bytes) != *id {
                return Err(Error::Invalid(
                    "object ID does not match canonical envelope".into(),
                ));
            }
            decode_object(bytes)?;
            if let Some(previous) = unique.insert(*id, *bytes) {
                if previous != *bytes {
                    return Err(Error::Invalid("SQLite batch object collision".into()));
                }
            }
        }

        let tx = begin_immediate_with_retry(conn)?;
        {
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO loom_storage_objects(oid,envelope) VALUES(?1,?2)",
            )?;
            for (id, bytes) in &unique {
                insert.execute(params![id.to_string(), bytes])?;
            }
        }
        {
            let mut query = tx.prepare(READ_OBJECT_SQL)?;
            for (id, bytes) in &unique {
                if Self::read_bounded(&mut query, *id, MAX_OBJECT_SIZE + 15)?.as_slice() != *bytes {
                    return Err(Error::Invalid("SQLite immutable object collision".into()));
                }
            }
        }
        tx.commit()?;
        Ok(())
    }
}

impl ObjectBackend for SqliteObjectBackend {
    fn open_read_session(&self) -> Result<Option<Arc<dyn ObjectReadSession>>> {
        struct Reader(Mutex<Connection>);
        impl ObjectReadSession for Reader {
            fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
                let conn = self
                    .0
                    .lock()
                    .map_err(|_| Error::Invalid("SQLite reader lock poisoned".into()))?;
                // Reuse the parsed query, never its rows or a read transaction.
                // The statement is reset before returning to the cache.
                let mut query = conn.prepare_cached(READ_OBJECT_SQL)?;
                SqliteObjectBackend::read_bounded(&mut query, id, MAX_OBJECT_SIZE + 15)
            }
        }
        Ok(Some(Arc::new(Reader(Mutex::new(self.read_connection()?)))))
    }
    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            durable_writes: true,
            conditional_create: true,
            streaming_reads: false,
            streaming_writes: false,
            list_objects: true,
        }
    }
    fn get(&self, id: ObjectId) -> Result<Vec<u8>> {
        // Ordinary immutable queries never need a write-capable handle or the
        // per-connection synchronous write policy. Lazily reuse a small bounded
        // query-only pool; publication acknowledgment continues to use the
        // separate fresh live-read path below.
        self.scalar_read(|connection| Self::read(connection, id))
    }
    fn get_batch_live(&self, ids: &[ObjectId], max_bytes: usize) -> Result<Option<Vec<Vec<u8>>>> {
        if ids.len() > crate::store::MAX_PUBLICATION_OBJECTS
            || (ids.len() > 1 && max_bytes > crate::store::MAX_PUBLICATION_BYTES)
            || (ids.len() == 1 && max_bytes > MAX_OBJECT_SIZE + 15)
        {
            return Err(Error::Invalid(
                "live object batch exceeds window bounds".into(),
            ));
        }
        if ids.is_empty() {
            return Ok(Some(Vec::new()));
        }
        // Fresh per-call connection, no retained read session or transaction.
        // Each SELECT finishes before the next: committed rows are read live,
        // while one prepared statement amortizes parsing within this call.
        let before = std::fs::symlink_metadata(&self.path)?;
        let conn = self.read_connection()?;
        let mut query = conn.prepare(READ_OBJECT_SQL)?;
        let mut remaining = max_bytes;
        let mut objects = Vec::with_capacity(ids.len());
        for id in ids {
            let bytes = Self::read_bounded(&mut query, *id, remaining)?;
            remaining -= bytes.len();
            objects.push(bytes);
        }
        let after = std::fs::symlink_metadata(&self.path)?;
        if !after.is_file() || after.file_type().is_symlink() {
            return Err(Error::Invalid(
                "SQLite object database changed during readback".into(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if before.dev() != after.dev() || before.ino() != after.ino() {
                return Err(Error::Invalid(
                    "SQLite object database changed during readback".into(),
                ));
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if before.creation_time() != after.creation_time() {
                return Err(Error::Invalid(
                    "SQLite object database changed during readback".into(),
                ));
            }
        }
        Ok(Some(objects))
    }
    fn put_if_absent(&self, id: ObjectId, bytes: &[u8]) -> Result<()> {
        if object_id(bytes) != id {
            return Err(Error::Invalid(
                "object ID does not match canonical envelope".into(),
            ));
        }
        decode_object(bytes)?;
        let _gate = write_gate(&self.path)?;
        let conn = self.connection()?;
        let tx = begin_immediate_with_retry(&conn)?;
        tx.execute(
            "INSERT OR IGNORE INTO loom_storage_objects(oid,envelope) VALUES(?1,?2)",
            params![id.to_string(), bytes],
        )?;
        if Self::read(&tx, id)? != bytes {
            return Err(Error::Invalid("SQLite immutable object collision".into()));
        }
        tx.commit()?;
        Ok(())
    }
    fn put_batch_if_absent(&self, objects: &[(ObjectId, &[u8])]) -> Result<bool> {
        let _gate = write_gate(&self.path)?;
        let mut conn = self.connection()?;
        Self::write_batch(&mut conn, objects)?;
        Ok(true)
    }
    fn open_write_session(&self) -> Result<Option<Box<dyn ObjectWriteSession>>> {
        let identity = DatabaseIdentity::read(&self.path)?;
        let connection = self.connection()?;
        if DatabaseIdentity::read(&self.path)? != identity {
            return Err(Error::Invalid(
                "SQLite object database changed while opening write session".into(),
            ));
        }
        Ok(Some(Box::new(SqliteWriteSession {
            path: self.path.clone(),
            identity,
            connection,
        })))
    }
    fn exists(&self, id: ObjectId) -> Result<bool> {
        match self.get(id) {
            Ok(_) => Ok(true),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
    fn list_ids(&self) -> Result<Vec<ObjectId>> {
        self.scalar_read(|connection| {
            let mut statement =
                connection.prepare_cached("SELECT oid FROM loom_storage_objects ORDER BY oid")?;
            let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
            rows.map(|row| ObjectId::parse(&row?)).collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{encode_object, BLOB};
    use std::sync::Arc;

    #[test]
    fn protocol_lock_failures_are_bounded_retry_candidates() {
        assert!(is_retryable_lock_code(ErrorCode::FileLockingProtocolFailed));
        assert!(is_retryable_lock_code(ErrorCode::DatabaseBusy));
        assert!(is_retryable_lock_code(ErrorCode::DatabaseLocked));
        assert!(!is_retryable_lock_code(ErrorCode::DatabaseCorrupt));
    }

    #[test]
    fn bundled_sqlite_runtime_contains_wal_reset_fix() {
        const EXPECTED_VERSION: &str = "3.51.3";
        const EXPECTED_SOURCE_ID: &str =
            "2026-03-13 10:38:09 737ae4a34738ffa0c3ff7f9bb18df914dd1cad163f28fd6b6e114a344fe6d618";

        let connection = Connection::open_in_memory().unwrap();
        let source_id: String = connection
            .query_row("SELECT sqlite_source_id()", [], |row| row.get(0))
            .unwrap();
        println!(
            "linked SQLite runtime={} source_id={source_id}",
            rusqlite::version()
        );
        assert_eq!(rusqlite::version(), EXPECTED_VERSION);
        assert_eq!(source_id, EXPECTED_SOURCE_ID);
    }

    #[test]
    fn concurrent_initializers_can_open_one_new_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects.sqlite");
        let barrier = Arc::new(std::sync::Barrier::new(12));
        let workers: Vec<_> = (0..12)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let backend = SqliteObjectBackend::open(path).unwrap();
                    let bytes = encode_object(BLOB, b"initialized together").unwrap();
                    backend.put_if_absent(object_id(&bytes), &bytes).unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(
            SqliteObjectBackend::open(path)
                .unwrap()
                .list_ids()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn concurrent_create_is_immutable_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects.sqlite");
        let backend = Arc::new(SqliteObjectBackend::open(&path).unwrap());
        let bytes = encode_object(BLOB, b"same bytes").unwrap();
        let id = object_id(&bytes);
        assert!(!backend.exists(id).unwrap());
        let writers: Vec<_> = (0..8)
            .map(|_| {
                let backend = backend.clone();
                let bytes = bytes.clone();
                std::thread::spawn(move || backend.put_if_absent(id, &bytes).unwrap())
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        drop(backend);
        let reopened = SqliteObjectBackend::open(&path).unwrap();
        assert_eq!(reopened.list_ids().unwrap(), vec![id]);
        assert_eq!(reopened.get(id).unwrap(), bytes);
        assert!(reopened.put_if_absent(ObjectId([0; 32]), &bytes).is_err());
    }

    #[test]
    fn corruption_is_reported_and_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects.sqlite");
        let backend = SqliteObjectBackend::open(&path).unwrap();
        let bytes = encode_object(BLOB, b"original").unwrap();
        let id = object_id(&bytes);
        backend.put_if_absent(id, &bytes).unwrap();
        let conn = Connection::open(path).unwrap();
        conn.execute(
            "UPDATE loom_storage_objects SET envelope=?1",
            [b"corrupt".as_slice()],
        )
        .unwrap();
        assert!(backend.get(id).is_err());
        assert!(backend.exists(id).is_err());
        assert!(backend.put_if_absent(id, &bytes).is_err());
        let actual: Vec<u8> = conn
            .query_row("SELECT envelope FROM loom_storage_objects", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(actual, b"corrupt");
    }

    #[test]
    fn live_batch_reads_preserve_order_duplicates_budget_and_freshness() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects.sqlite");
        let backend = SqliteObjectBackend::open(&path).unwrap();
        let bytes: Vec<_> = (0..3)
            .map(|n| encode_object(BLOB, format!("live-{n}").as_bytes()).unwrap())
            .collect();
        let entries: Vec<_> = bytes.iter().map(|b| (object_id(b), b.as_slice())).collect();
        backend.put_batch_if_absent(&entries).unwrap();
        let order = [2, 0, 2, 1];
        let ids: Vec<_> = order.iter().map(|&n| entries[n].0).collect();
        let expected: Vec<_> = order.iter().map(|&n| bytes[n].clone()).collect();
        let budget = expected.iter().map(Vec::len).sum::<usize>();
        assert_eq!(
            backend.get_batch_live(&ids, budget).unwrap(),
            Some(expected)
        );
        assert!(backend.get_batch_live(&ids, budget - 1).is_err());
        assert!(backend.get_batch_live(&vec![ids[0]; 129], budget).is_err());
        assert!(backend
            .get_batch_live(&ids, crate::store::MAX_PUBLICATION_BYTES + 1)
            .is_err());

        // A completed batch cannot retain a stale connection or payload cache.
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE loom_storage_objects SET envelope=?1 WHERE oid=?2",
            params![b"corrupt".as_slice(), ids[0].to_string()],
        )
        .unwrap();
        assert!(backend.get_batch_live(&ids, budget).is_err());
        conn.execute(
            "DELETE FROM loom_storage_objects WHERE oid=?1",
            [ids[0].to_string()],
        )
        .unwrap();
        assert!(matches!(backend.get_batch_live(&ids, budget),
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound));
    }

    #[test]
    fn cached_reader_observes_commits_deletions_and_recovers_after_read_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects.sqlite");
        let backend = SqliteObjectBackend::open(&path).unwrap();
        let bytes = encode_object(BLOB, b"first").unwrap();
        let id = object_id(&bytes);
        backend.put_if_absent(id, &bytes).unwrap();
        let reader = backend.open_read_session().unwrap().unwrap();
        assert_eq!(reader.get(id).unwrap(), bytes);

        let mut writer = Connection::open(&path).unwrap();
        let tx = writer.transaction().unwrap();
        tx.execute(
            "UPDATE loom_storage_objects SET envelope=?1 WHERE oid=?2",
            params![b"corrupt".as_slice(), id.to_string()],
        )
        .unwrap();
        assert_eq!(reader.get(id).unwrap(), bytes);
        tx.commit().unwrap();
        assert!(reader.get(id).is_err());
        assert!(backend.get(id).is_err());
        writer
            .execute(
                "UPDATE loom_storage_objects SET envelope=?1 WHERE oid=?2",
                params![&bytes, id.to_string()],
            )
            .unwrap();
        assert_eq!(reader.get(id).unwrap(), bytes);

        let second = encode_object(BLOB, b"second").unwrap();
        let second_id = object_id(&second);
        backend.put_if_absent(second_id, &second).unwrap();
        assert_eq!(reader.get(second_id).unwrap(), second);
        writer
            .execute(
                "DELETE FROM loom_storage_objects WHERE oid=?1",
                [id.to_string()],
            )
            .unwrap();
        assert!(matches!(reader.get(id), Err(Error::Io(e))
            if e.kind() == std::io::ErrorKind::NotFound));
        assert!(matches!(backend.get(id), Err(Error::Io(e))
            if e.kind() == std::io::ErrorKind::NotFound));
        assert_eq!(reader.get(second_id).unwrap(), second);
        assert_eq!(backend.list_ids().unwrap(), vec![second_id]);
    }

    #[test]
    fn live_readback_allows_one_oversized_canonical_object_without_unbounded_windows() {
        let dir = tempfile::tempdir().unwrap();
        let backend = SqliteObjectBackend::open(dir.path().join("objects.sqlite")).unwrap();
        let bytes =
            encode_object(BLOB, &vec![0x5a; crate::store::MAX_PUBLICATION_BYTES + 1]).unwrap();
        let id = object_id(&bytes);
        backend.put_if_absent(id, &bytes).unwrap();
        assert_eq!(
            backend.get_batch_live(&[id], bytes.len()).unwrap(),
            Some(vec![bytes.clone()])
        );
        assert!(backend.get_batch_live(&[id], bytes.len() - 1).is_err());
        assert!(backend.get_batch_live(&[id, id], bytes.len() * 2).is_err());
    }

    #[test]
    fn batch_failure_rolls_back_every_new_object_and_retry_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects.sqlite");
        let backend = SqliteObjectBackend::open(&path).unwrap();
        let envelopes: Vec<_> = (0..4)
            .map(|index| encode_object(BLOB, format!("batch-{index}").as_bytes()).unwrap())
            .collect();
        let objects: Vec<_> = envelopes
            .iter()
            .map(|bytes| (object_id(bytes), bytes.as_slice()))
            .collect();

        let rejected = objects[2].0;
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!(
            "CREATE TRIGGER reject_batch_insert BEFORE INSERT ON loom_storage_objects
             WHEN NEW.oid='{}' BEGIN SELECT RAISE(ABORT, 'injected batch failure'); END;",
            rejected
        ))
        .unwrap();
        let mut writer = backend.open_write_session().unwrap().unwrap();
        assert!(writer.put_batch_if_absent(&objects).is_err());
        assert!(backend.list_ids().unwrap().is_empty());

        conn.execute_batch("DROP TRIGGER reject_batch_insert;")
            .unwrap();
        assert!(writer.put_batch_if_absent(&objects).unwrap());
        assert!(writer.put_batch_if_absent(&objects).unwrap());
        assert_eq!(backend.list_ids().unwrap(), {
            let mut ids: Vec<_> = objects.iter().map(|(id, _)| *id).collect();
            ids.sort_unstable();
            ids
        });
        for (id, bytes) in objects {
            assert_eq!(backend.get(id).unwrap(), bytes);
        }
    }

    #[test]
    fn write_session_rejects_a_changed_database_identity() {
        let dir = tempfile::tempdir().unwrap();
        let original_path = dir.path().join("original.sqlite");
        let replacement_path = dir.path().join("replacement.sqlite");
        let original = SqliteObjectBackend::open(&original_path).unwrap();
        let replacement = SqliteObjectBackend::open(&replacement_path).unwrap();
        let bytes = encode_object(BLOB, b"must not reach another database").unwrap();
        let id = object_id(&bytes);

        // Model a path replacement without relying on platform-specific rules
        // for renaming an open database file (Windows can deny that rename).
        let mut writer = SqliteWriteSession {
            path: replacement_path,
            identity: DatabaseIdentity::read(&original_path).unwrap(),
            connection: original.connection().unwrap(),
        };
        assert!(writer
            .put_batch_if_absent(&[(id, bytes.as_slice())])
            .is_err());
        assert!(replacement.list_ids().unwrap().is_empty());
        assert!(original.list_ids().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn write_session_does_not_acknowledge_an_unlinked_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects.sqlite");
        let backend = SqliteObjectBackend::open(&path).unwrap();
        let mut writer = backend.open_write_session().unwrap().unwrap();
        std::fs::remove_file(&path).unwrap();
        let bytes = encode_object(BLOB, b"unlinked writer").unwrap();
        assert!(writer
            .put_batch_if_absent(&[(object_id(&bytes), bytes.as_slice())])
            .is_err());
        assert!(!path.exists());
    }

    #[test]
    fn batch_collision_rolls_back_new_siblings_without_replacing_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects.sqlite");
        let backend = SqliteObjectBackend::open(&path).unwrap();
        let existing = encode_object(BLOB, b"existing").unwrap();
        let existing_id = object_id(&existing);
        backend.put_if_absent(existing_id, &existing).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE loom_storage_objects SET envelope=?1 WHERE oid=?2",
            params![b"corrupt".as_slice(), existing_id.to_string()],
        )
        .unwrap();
        let sibling = encode_object(BLOB, b"new sibling").unwrap();
        let sibling_id = object_id(&sibling);
        assert!(backend
            .put_batch_if_absent(&[
                (sibling_id, sibling.as_slice()),
                (existing_id, existing.as_slice()),
            ])
            .is_err());
        assert!(!backend.list_ids().unwrap().contains(&sibling_id));
        let actual: Vec<u8> = conn
            .query_row(
                "SELECT envelope FROM loom_storage_objects WHERE oid=?1",
                [existing_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(actual, b"corrupt");
    }

    #[test]
    fn volatile_or_missing_database_cannot_be_acknowledged() {
        assert!(SqliteObjectBackend::open(":memory:").is_err());
        assert!(SqliteObjectBackend::open("").is_err());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects.sqlite");
        let backend = SqliteObjectBackend::open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let bytes = encode_object(BLOB, b"never acknowledged").unwrap();
        assert!(backend.put_if_absent(object_id(&bytes), &bytes).is_err());
        assert!(backend.get(object_id(&bytes)).is_err());
        assert!(backend.list_ids().is_err());
        assert!(backend
            .get_batch_live(&[object_id(&bytes)], bytes.len())
            .is_err());
        assert!(!path.exists());
    }

    #[test]
    fn ordinary_queries_use_query_only_connections_and_reject_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects.sqlite");
        let backend = SqliteObjectBackend::open(&path).unwrap();
        let bytes = encode_object(BLOB, b"read-only query").unwrap();
        let id = object_id(&bytes);
        backend.put_if_absent(id, &bytes).unwrap();

        let conn = backend.read_connection().unwrap();
        assert_eq!(
            conn.query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(backend.get(id).unwrap(), bytes);
        assert_eq!(backend.list_ids().unwrap(), vec![id]);

        let replacement_path = dir.path().join("replacement.sqlite");
        let replacement = SqliteObjectBackend::open(&replacement_path).unwrap();
        let substituted = SqliteObjectBackend {
            path: replacement_path,
            identity: backend.identity,
            scalar_readers: ScalarReaderPool::new(),
        };
        assert!(substituted.get(id).is_err());
        assert!(substituted.list_ids().is_err());
        assert!(replacement.list_ids().unwrap().is_empty());
    }

    #[test]
    fn scalar_reader_pool_allows_independent_reads_to_overlap() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(
            SqliteObjectBackend::open(dir.path().join("objects.sqlite")).unwrap(),
        );
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_first_tx, release_first_rx) = std::sync::mpsc::channel();
        let (release_second_tx, release_second_rx) = std::sync::mpsc::channel();

        let first_backend = Arc::clone(&backend);
        let first_entered = entered_tx.clone();
        let first = std::thread::spawn(move || {
            first_backend.scalar_read(|_| {
                first_entered.send(()).unwrap();
                let _ = release_first_rx.recv();
                Ok(())
            })
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();

        let second_backend = Arc::clone(&backend);
        let second = std::thread::spawn(move || {
            second_backend.scalar_read(|_| {
                entered_tx.send(()).unwrap();
                let _ = release_second_rx.recv();
                Ok(())
            })
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("a second scalar reader must not wait behind the first");

        release_first_tx.send(()).unwrap();
        release_second_tx.send(()).unwrap();
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();

        let state = backend.scalar_readers.state.lock().unwrap();
        assert_eq!(state.opened, 2);
        assert_eq!(state.idle.len(), 2);
    }

    #[test]
    fn existing_open_never_creates_or_bootstraps_storage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.sqlite");
        assert!(SqliteObjectBackend::open_existing(&path).is_err());
        assert!(!path.exists());
        Connection::open(&path).unwrap();
        assert!(SqliteObjectBackend::open_existing(&path).is_err());
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}
