//! What the window and the tray share.
//!
//! One [`Capture`] and one read connection, both behind the application state
//! Tauri hands to every command. The capture owns the process's single write
//! handle ([ADR-0007]); this connection reads alongside it under WAL.
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use toolog_cli::capture::Capture;
use toolog_cli::prefs::Prefs;
use toolog_core::{Connection, Db};

/// Application state, managed by Tauri and shared with the tray.
pub(crate) struct AppState {
    pub(crate) db_path: PathBuf,
    /// The running pipeline. `None` only while shutting down.
    capture: Mutex<Option<Capture>>,
    /// A read connection, separate from the writer's.
    reader: Mutex<Connection>,
    /// The notification switches (task 6.12), shared with the live sink so it
    /// can read them without touching disk on every arriving call.
    prefs: Arc<RwLock<Prefs>>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("db_path", &self.db_path)
            .finish_non_exhaustive()
    }
}

impl AppState {
    pub(crate) fn new(
        db_path: PathBuf,
        capture: Capture,
        prefs: Arc<RwLock<Prefs>>,
    ) -> anyhow::Result<Arc<Self>> {
        let reader = Db::open(&db_path)?.into_connection();
        Ok(Arc::new(Self {
            db_path,
            capture: Mutex::new(Some(capture)),
            reader: Mutex::new(reader),
            prefs,
        }))
    }

    /// The notification switches as they stand.
    pub(crate) fn prefs(&self) -> Prefs {
        self.prefs.read().map(|p| *p).unwrap_or_default()
    }

    /// Write the switches to disk and to the live sink's copy together, so the
    /// two cannot disagree about what the user asked for.
    pub(crate) fn set_prefs(&self, next: Prefs) -> anyhow::Result<Prefs> {
        toolog_cli::prefs::save(next)?;
        next.apply();
        let mut guard = self
            .prefs
            .write()
            .map_err(|_| anyhow::anyhow!("the preferences lock is poisoned"))?;
        *guard = next;
        Ok(next)
    }

    /// Run a query against the read connection.
    ///
    /// Serialized by a mutex rather than pooled: the Phase 1 measurements put
    /// every query under 3 ms on the real corpus, so contention is not yet a
    /// problem worth a pool. Revisit when the timeline is virtualized over a
    /// much larger store.
    pub(crate) fn read<T>(
        &self,
        f: impl FnOnce(&Connection) -> toolog_core::Result<T>,
    ) -> anyhow::Result<T> {
        let conn = self
            .reader
            .lock()
            .map_err(|_| anyhow::anyhow!("the read connection is poisoned"))?;
        Ok(f(&conn)?)
    }

    /// Do something with the running capture.
    pub(crate) fn with_capture<T>(
        &self,
        f: impl FnOnce(&Capture) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let guard = self
            .capture
            .lock()
            .map_err(|_| anyhow::anyhow!("the capture lock is poisoned"))?;
        let capture = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("capture is shutting down"))?;
        f(capture)
    }

    /// Stop capture and release the writer. Idempotent.
    pub(crate) fn shutdown(&self) {
        let taken = self.capture.lock().ok().and_then(|mut g| g.take());
        if let Some(capture) = taken {
            capture.shutdown();
        }
    }
}
