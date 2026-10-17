//! Loom: an independent content-addressed version control engine.
//! Native storage and history have no Git runtime dependency.
pub mod cloud_storage;
#[cfg(unix)]
mod filesystem_read;
mod format;
mod ignore;
pub mod memory_storage;
mod read_pack;
pub mod replicated_storage;
mod repository;
mod snapshot;
pub mod sqlite_object_storage;
pub mod sqlite_ref_storage;
pub mod storage_backend;
mod store;
mod text_merge;
mod tree;
mod tree_index;
mod workers;
mod worktree_scan;

pub use format::{Change, ChangeKind, Changeset, ObjectId};
pub use read_pack::ReadPackStats;
pub use repository::{FileDiff, Repository, StatusEntry, StatusKind};
pub use snapshot::{
    Snapshot, SnapshotFiles, SnapshotStats, SparseExportStats, DEFAULT_BULK_BYTES,
    DEFAULT_SNAPSHOT_CACHE_BYTES, MAX_BULK_BYTES, MAX_BULK_PATHS,
};
pub use store::FilesystemObjectBackend;
pub use tree::{NativeTree, TreeEntry, TreeEntryKind};
pub use worktree_scan::WorktreeScanOptions;

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
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e)
    }
}
