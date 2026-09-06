//! What the window and the tray share.
//!
//! One [`Capture`] and **two** read connections, behind the application state
//! Tauri hands to every command. The capture owns the process's single write
//! handle ([ADR-0007]); these read alongside it under WAL.
//!
//! The second connection is the risk review's (task 11.4). The comment on
//! [`AppState::read`] said to revisit the single serialized reader when a query
//! stopped being cheap, and the review is that query: it was 2.3 seconds on the
//! owner's store, which the timeline spent waiting behind. WAL already permits
//! concurrent readers, so this is a second `Connection` rather than a pool.
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// The risk review's own connection (task 11.4).
    ///
    /// Separate so a slow review cannot hold the timeline behind the shared
    /// mutex — and *necessary* for the memo below, because `PRAGMA
    /// data_version` reports commits by **other** connections and would never
    /// move on the one doing the writing.
    risk_reader: Mutex<Connection>,
    /// The last risk review, and what would make it stale (task 11.3).
    risk_memo: Mutex<Option<RiskMemo>>,
    /// Bumped by every dismissal and restore.
    ///
    /// A dismissal is a commit like any other, so `data_version` catches it
    /// too — verified, not assumed. This is kept because it is exact and free:
    /// the command that writes the judgement is the one that invalidates the
    /// memo, rather than the memo inferring it from a side effect.
    dismissal_counter: AtomicU64,
    /// The notification switches (task 6.12), shared with the live sink so it
    /// can read them without touching disk on every arriving call.
    prefs: Arc<RwLock<Prefs>>,
}

/// A memoized risk review and the three cheap facts that can retire it.
///
/// All three are read in well under a millisecond, which is the point: the
/// alternative to re-running twelve rules is not a cache that might be wrong,
/// it is a watermark that says so.
pub(crate) struct RiskMemo {
    pub(crate) data_version: i64,
    /// The user rules file's mtime, and `None` when there is no such file —
    /// which is itself a state that can change.
    pub(crate) rules_mtime: Option<std::time::SystemTime>,
    pub(crate) dismissals: u64,
    pub(crate) review: crate::commands::RiskReview,
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
        let risk_reader = Db::open(&db_path)?.into_connection();
        Ok(Arc::new(Self {
            db_path,
            capture: Mutex::new(Some(capture)),
            reader: Mutex::new(reader),
            risk_reader: Mutex::new(risk_reader),
            risk_memo: Mutex::new(None),
            dismissal_counter: AtomicU64::new(0),
            prefs,
        }))
    }

    /// Run the risk evaluation on its own connection.
    pub(crate) fn read_risk<T>(
        &self,
        f: impl FnOnce(&Connection) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let conn = self
            .risk_reader
            .lock()
            .map_err(|_| anyhow::anyhow!("the risk connection is poisoned"))?;
        f(&conn)
    }

    /// The store's commit counter, as this connection sees it.
    ///
    /// `PRAGMA data_version` moves on any commit by *another* connection, which
    /// is exactly the set of events that can invalidate a review: an ingest, an
    /// OTEL arrival completing a row the transcript created, a `toolog purge`
    /// in another process. Measured against the obvious alternatives:
    /// `max(rowid)` misses the update, and the writer's update hook ignores
    /// deletes and only exists when a live sink does. Asserted in
    /// `toolog-core/tests/roundtrip.rs`, not assumed.
    pub(crate) fn data_version(&self) -> anyhow::Result<i64> {
        self.read_risk(|c| Ok(c.query_row("PRAGMA data_version", [], |r| r.get(0))?))
    }

    /// What the store looked like before a review is computed.
    ///
    /// Taken **before** the evaluation, never after. A write that lands while
    /// the rules are running would otherwise be stamped into the memo as
    /// though the answer already accounted for it, and the memo would look
    /// fresh while describing the store as it was. Reading it first can only
    /// make the memo expire early, which costs a recomputation; reading it
    /// last can make it serve an answer that is wrong.
    pub(crate) fn risk_watermark(&self) -> anyhow::Result<(i64, u64)> {
        Ok((
            self.data_version()?,
            self.dismissal_counter.load(Ordering::SeqCst),
        ))
    }

    /// A judgement was recorded. The next review is computed rather than reused.
    pub(crate) fn note_dismissal(&self) {
        self.dismissal_counter.fetch_add(1, Ordering::SeqCst);
    }

    /// The memoized review, if nothing that would change it has happened.
    pub(crate) fn cached_risk(
        &self,
        rules_mtime: Option<std::time::SystemTime>,
    ) -> Option<crate::commands::RiskReview> {
        let version = self.data_version().ok()?;
        let dismissals = self.dismissal_counter.load(Ordering::SeqCst);
        let guard = self.risk_memo.lock().ok()?;
        let memo = guard.as_ref()?;
        (memo.data_version == version
            && memo.rules_mtime == rules_mtime
            && memo.dismissals == dismissals)
            .then(|| memo.review.clone())
    }

    /// Remember a review against the watermark taken before it was computed.
    pub(crate) fn remember_risk(
        &self,
        watermark: (i64, u64),
        rules_mtime: Option<std::time::SystemTime>,
        review: &crate::commands::RiskReview,
    ) {
        let (data_version, dismissals) = watermark;
        if let Ok(mut guard) = self.risk_memo.lock() {
            *guard = Some(RiskMemo {
                data_version,
                rules_mtime,
                dismissals,
                review: review.clone(),
            });
        }
    }

    /// The notification switches as they stand.
    pub(crate) fn prefs(&self) -> Prefs {
        self.prefs.read().map(|p| p.clone()).unwrap_or_default()
    }

    /// Write the switches to disk and to the live sink's copy together, so the
    /// two cannot disagree about what the user asked for.
    pub(crate) fn set_prefs(&self, next: Prefs) -> anyhow::Result<Prefs> {
        toolog_cli::prefs::save(&next)?;
        next.apply();
        let mut guard = self
            .prefs
            .write()
            .map_err(|_| anyhow::anyhow!("the preferences lock is poisoned"))?;
        *guard = next.clone();
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
