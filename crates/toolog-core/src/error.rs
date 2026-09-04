//! Error type for the storage core.

use std::path::PathBuf;

/// Anything that can go wrong in `toolog-core`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("could not determine the platform data directory")]
    NoDataDir,

    #[error("could not create data directory {path}: {source}")]
    CreateDataDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "database schema is at version {found}, but this build only knows up to {known}; \
         it was written by a newer toolog"
    )]
    SchemaFromTheFuture { found: u32, known: u32 },

    #[error("migration {version} ({name}) failed: {source}")]
    Migration {
        version: u32,
        name: &'static str,
        #[source]
        source: rusqlite::Error,
    },
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
