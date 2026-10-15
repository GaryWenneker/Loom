//! Durable SQLite compare-and-swap reference authority.
//! Independent from the repository's worktree transaction journal.
use crate::storage_backend::RefBackend;
use crate::{Error, ObjectId, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::path::Path;

pub struct SqliteRefBackend {
    path: std::path::PathBuf,
}

impl SqliteRefBackend {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS loom_storage_refs (
                 name TEXT PRIMARY KEY NOT NULL,
                 oid TEXT NOT NULL
             );",
        )?;
        Ok(Self { path })
    }

    fn connection(&self) -> Result<Connection> {
        let conn = Connection::open(&self.path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(conn)
    }
}

impl RefBackend for SqliteRefBackend {
    fn read_ref(&self, name: &str) -> Result<Option<ObjectId>> {
        validate_name(name)?;
        let conn = self.connection()?;
        let value: Option<String> = conn.query_row(
            "SELECT oid FROM loom_storage_refs WHERE name=?1",
            [name], |row| row.get(0),
        ).optional()?;
        value.map(|v| ObjectId::parse(&v)).transpose()
    }

    fn compare_and_swap(
        &self,
        name: &str,
        expected: Option<ObjectId>,
        replacement: Option<ObjectId>,
    ) -> Result<bool> {
        validate_name(name)?;
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<String> = tx.query_row(
            "SELECT oid FROM loom_storage_refs WHERE name=?1",
            [name], |row| row.get(0),
        ).optional()?;
        let parsed = current.map(|v| ObjectId::parse(&v)).transpose()?;
        if parsed != expected { return Ok(false); }
        match replacement {
            Some(next) => {
                tx.execute(
                    "INSERT INTO loom_storage_refs(name,oid) VALUES(?1,?2)
                     ON CONFLICT(name) DO UPDATE SET oid=excluded.oid",
                    params![name, next.to_string()],
                )?;
            }
            None => { tx.execute("DELETE FROM loom_storage_refs WHERE name=?1", [name])?; }
        }
        tx.commit()?;
        Ok(true)
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 1024
        || name.starts_with('/') || name.ends_with('/')
        || name.split('/').any(|part| part.is_empty() || part == "." || part == "..")
        || name.chars().any(|c| c.is_control() || c == '\\')
    {
        return Err(Error::Invalid("invalid reference name".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn durable_cas_survives_reopen_and_rejects_stale_writers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refs.sqlite");
        let refs = SqliteRefBackend::open(&path).unwrap();
        let a = ObjectId([1; 32]);
        let b = ObjectId([2; 32]);
        assert!(refs.compare_and_swap("main", None, Some(a)).unwrap());
        assert!(!refs.compare_and_swap("main", None, Some(b)).unwrap());
        drop(refs);
        let refs = SqliteRefBackend::open(&path).unwrap();
        assert_eq!(refs.read_ref("main").unwrap(), Some(a));
        assert!(refs.compare_and_swap("main", Some(a), Some(b)).unwrap());
        assert_eq!(refs.read_ref("main").unwrap(), Some(b));
    }

    #[test]
    fn concurrent_connections_have_one_cas_winner() {
        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("refs.sqlite"));
        SqliteRefBackend::open(path.as_path()).unwrap();
        let workers: Vec<_> = (1..=8).map(|n| {
            let path = Arc::clone(&path);
            std::thread::spawn(move || {
                let refs = SqliteRefBackend::open(path.as_path()).unwrap();
                refs.compare_and_swap("main", None, Some(ObjectId([n; 32]))).unwrap()
            })
        }).collect();
        assert_eq!(workers.into_iter().map(|w| w.join().unwrap()).filter(|won| *won).count(), 1);
    }
}
