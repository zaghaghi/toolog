//! Connection handling: paths, pragmas, migration on open.
//!
//! Per [ADR-0003] the Rust core owns the only handle. Nothing else in the
//! workspace opens the database, and the frontend reaches it through typed
//! commands rather than SQL.
//!
//! [ADR-0003]: ../../../docs/adr/0003-sqlite-as-the-embedded-store.md

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;

use crate::constants::APP_NAME;
use crate::error::{Error, Result};
use crate::migrations;

/// How long a writer waits on a locked database before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// An open, migrated database.
#[derive(Debug)]
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open (creating if absent) the database at `path` and migrate it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::CreateDataDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        Self::from_connection(Connection::open(path)?)
    }

    /// Open the database at [`default_path`].
    pub fn open_default() -> Result<Self> {
        Self::open(default_path()?)
    }

    /// An in-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        apply_pragmas(&conn)?;
        migrations::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Borrow the underlying connection.
    ///
    /// Within the crate this is how the query and projection modules work. It is
    /// public so tests and the measurement harness can reach it; callers outside
    /// `toolog-core` should prefer the typed query layer ([ADR-0003]).
    ///
    /// [ADR-0003]: ../../../docs/adr/0003-sqlite-as-the-embedded-store.md
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Consume the wrapper and return the connection.
    pub fn into_connection(self) -> Connection {
        self.conn
    }
}

/// Apply the connection settings from Phase 1.1.
fn apply_pragmas(conn: &Connection) -> Result<()> {
    // WAL lets the UI read while ingestion writes. It is a no-op (and reports
    // "memory") for in-memory databases, which is fine.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // NORMAL is the standard pairing with WAL: durable across process crashes,
    // and only at risk from an OS-level crash mid-write.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(())
}

/// The platform data directory for the application's own files.
///
/// macOS `~/Library/Application Support/toolog`, Linux `~/.local/share/toolog`,
/// Windows `%APPDATA%\toolog`. Resolved through `directories` from day one so
/// the Phase 8 Linux build needs no path rework.
pub fn data_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", APP_NAME).ok_or(Error::NoDataDir)?;
    Ok(dirs.data_dir().to_path_buf())
}

/// Default location of the database file.
pub fn default_path() -> Result<PathBuf> {
    Ok(data_dir()?.join(format!("{APP_NAME}.db")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pragmas_are_applied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(dir.path().join("t.db")).expect("open");

        let journal: String = db
            .conn()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .expect("journal_mode");
        assert_eq!(journal.to_lowercase(), "wal");

        let fk: i64 = db
            .conn()
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .expect("foreign_keys");
        assert_eq!(fk, 1);
    }

    #[test]
    fn open_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("a").join("b").join("t.db");
        Db::open(&nested).expect("open nested");
        assert!(nested.exists());
    }

    #[test]
    fn default_path_is_under_the_app_name() {
        let p = default_path().expect("default path");
        assert!(p.to_string_lossy().contains(APP_NAME));
        assert!(p.ends_with(format!("{APP_NAME}.db")));
    }
}
