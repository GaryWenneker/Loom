//! Immutable canonical objects in a durable SQLite database.
//! Object transactions finish before the repository publishes local branch refs.
use crate::format::{decode_object, object_id, MAX_OBJECT_SIZE};
use crate::storage_backend::{ObjectBackend, StoreCapabilities};
use crate::{Error, ObjectId, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::path::{Path, PathBuf};

pub struct SqliteObjectBackend {
    path: PathBuf,
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
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = checked_path(path.as_ref())?;
        let conn = Connection::open(&path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(Error::Invalid(
                "SQLite object backend requires WAL journal mode".into(),
            ));
        }
        conn.execute_batch(
            "PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS loom_storage_objects (
                 oid TEXT PRIMARY KEY NOT NULL CHECK(length(oid)=64),
                 envelope BLOB NOT NULL
             );",
        )?;
        Ok(Self { path })
    }

    fn connection(&self) -> Result<Connection> {
        checked_path(&self.path)?;
        // Never recreate a database lost/replaced after opening the provider.
        let conn = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA synchronous=FULL;")?;
        Ok(conn)
    }

    fn read(conn: &Connection, id: ObjectId) -> Result<Vec<u8>> {
        let row: Option<Option<Vec<u8>>> = conn
            .query_row(
                "SELECT CASE WHEN length(envelope)<=?2 THEN envelope ELSE NULL END
             FROM loom_storage_objects WHERE oid=?1",
                params![id.to_string(), (MAX_OBJECT_SIZE + 15) as i64],
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
                    "SQLite object exceeds envelope size limit: {id}"
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
}

impl ObjectBackend for SqliteObjectBackend {
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
        Self::read(&self.connection()?, id)
    }
    fn put_if_absent(&self, id: ObjectId, bytes: &[u8]) -> Result<()> {
        if object_id(bytes) != id {
            return Err(Error::Invalid(
                "object ID does not match canonical envelope".into(),
            ));
        }
        decode_object(bytes)?;
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
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
    fn exists(&self, id: ObjectId) -> Result<bool> {
        match self.get(id) {
            Ok(_) => Ok(true),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
    fn list_ids(&self) -> Result<Vec<ObjectId>> {
        let conn = self.connection()?;
        let mut statement = conn.prepare("SELECT oid FROM loom_storage_objects ORDER BY oid")?;
        let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|row| ObjectId::parse(&row?)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{encode_object, BLOB};
    use std::sync::Arc;

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
    fn volatile_or_missing_database_cannot_be_acknowledged() {
        assert!(SqliteObjectBackend::open(":memory:").is_err());
        assert!(SqliteObjectBackend::open("").is_err());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects.sqlite");
        let backend = SqliteObjectBackend::open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let bytes = encode_object(BLOB, b"never acknowledged").unwrap();
        assert!(backend.put_if_absent(object_id(&bytes), &bytes).is_err());
        assert!(!path.exists());
    }
}
