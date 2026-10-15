//! Loom: an independent content-addressed version control engine.
//! Native storage and history have no Git runtime dependency.
mod format;
pub mod replicated_storage;
pub mod cloud_storage;
mod ignore;
mod repository;
mod store;
pub mod storage_backend;
mod text_merge;
mod tree;

pub use format::{Change, ChangeKind, Changeset, ObjectId};
pub use repository::{FileDiff, Repository, StatusEntry, StatusKind};
pub use tree::{NativeTree, TreeEntry, TreeEntryKind};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Database(rusqlite::Error),
    Invalid(String),
    Conflict(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Database(e) => write!(f, "database error: {e}"),
            Self::Invalid(e) => write!(f, "invalid repository or object: {e}"),
            Self::Conflict(e) => write!(f, "concurrent update: {e}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self { Self::Database(e) }
}
