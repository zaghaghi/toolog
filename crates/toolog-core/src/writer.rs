//! The single database write handle ([ADR-0007]).
//!
//! One process, one writer. The OTLP receiver and the transcript tailer both
//! produce writes, and rather than give each its own connection and rediscover
//! SQLite lock contention, they submit closures to one thread that owns the
//! only write connection.
//!
//! # Backpressure is deliberate
//!
//! The queue is bounded. [`WriteHandle::try_submit`] fails rather than growing
//! without limit, so the OTLP handler can answer 503 and let the exporter
//! retry; [`WriteHandle::submit`] blocks, which is what a file tailer wants —
//! the transcript is still on disk, so waiting costs nothing and loses nothing.
//!
//! An audit tool that quietly drops records is worse than one that admits it
//! cannot keep up.
//!
//! # Telling the rest of the process what changed
//!
//! [`spawn_watching`] installs SQLite's update hook on the write connection
//! and reports, after each committed job, which `tool_call` rows it touched.
//! That is what turns the live view from a timer into a channel (task 6.9):
//! every row either lane writes passes through this one connection, so one
//! hook covers both without threading a callback through each projector.
//!
//! The rowids are collected during the job and delivered **after** it returns.
//! The hook itself fires before the commit, so a reader told about a row that
//! early would look for one it cannot see yet.
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use rusqlite::hooks::Action;

use crate::Db;

/// Work for the writer thread.
type Job = Box<dyn FnOnce(&Connection) + Send + 'static>;

/// Told which `tool_call` rows a committed job created or changed.
///
/// Runs on the writer thread, so it must not block: hand the rowids to a
/// channel and let something else do the reading.
pub type ChangeSink = Arc<dyn Fn(&[i64]) + Send + Sync>;

/// Why a submission did not reach the writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SubmitError {
    /// The queue is full. The caller decides: refuse, or wait.
    #[error("the write queue is full")]
    Full,
    /// The writer thread is gone — shutdown, or it panicked.
    #[error("the writer has stopped")]
    Stopped,
}

/// A cloneable handle to the process's one write connection.
///
/// The writer thread lives until every handle is dropped.
#[derive(Debug, Clone)]
pub struct WriteHandle {
    tx: SyncSender<Job>,
}

impl WriteHandle {
    /// Submit work, failing immediately if the queue is full.
    ///
    /// For producers that must not block — an HTTP handler answering an
    /// exporter that will retry anyway.
    pub fn try_submit(
        &self,
        job: impl FnOnce(&Connection) + Send + 'static,
    ) -> Result<(), SubmitError> {
        self.tx.try_send(Box::new(job)).map_err(|e| match e {
            TrySendError::Full(_) => SubmitError::Full,
            TrySendError::Disconnected(_) => SubmitError::Stopped,
        })
    }

    /// Submit work, waiting for room.
    ///
    /// For producers reading from a durable source, where waiting is free.
    pub fn submit(
        &self,
        job: impl FnOnce(&Connection) + Send + 'static,
    ) -> Result<(), SubmitError> {
        self.tx
            .send(Box::new(job))
            .map_err(|_| SubmitError::Stopped)
    }

    /// Submit work and wait for its result.
    ///
    /// Convenience for callers that need the return value — a backfill run, an
    /// on-demand re-projection. Do not call it from the writer thread.
    pub fn submit_blocking<T: Send + 'static>(
        &self,
        job: impl FnOnce(&Connection) -> T + Send + 'static,
    ) -> Result<T, SubmitError> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.submit(move |conn| {
            // A send failure means the caller stopped waiting, which is fine:
            // the work itself has already been done.
            let _ = tx.send(job(conn));
        })?;
        rx.recv().map_err(|_| SubmitError::Stopped)
    }
}

/// How many queued jobs before producers feel backpressure.
pub const DEFAULT_QUEUE_DEPTH: usize = 256;

/// Start the writer thread, taking ownership of the database.
///
/// The thread exits when the last [`WriteHandle`] is dropped.
pub fn spawn(db: Db) -> std::io::Result<WriteHandle> {
    spawn_with_depth(db, DEFAULT_QUEUE_DEPTH)
}

/// [`spawn`] with an explicit queue depth.
pub fn spawn_with_depth(db: Db, depth: usize) -> std::io::Result<WriteHandle> {
    spawn_inner(db, depth, None)
}

/// [`spawn`], plus a report of which `tool_call` rows each job touched.
///
/// The sink is called once per committed job, never for a job that wrote
/// nothing. See the module docs for why it is after the commit and not inside
/// the hook.
pub fn spawn_watching(db: Db, sink: ChangeSink) -> std::io::Result<WriteHandle> {
    spawn_inner(db, DEFAULT_QUEUE_DEPTH, Some(sink))
}

fn spawn_inner(db: Db, depth: usize, sink: Option<ChangeSink>) -> std::io::Result<WriteHandle> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<Job>(depth);

    std::thread::Builder::new()
        .name("toolog-writer".into())
        .spawn(move || {
            let conn = db.into_connection();

            // Records written before the integrity chain existed are linked in
            // now (task 7.6). Here because this thread owns the only write
            // connection, once per process, and it costs one indexed count when
            // there is nothing to do.
            match crate::chain::seal(&conn) {
                Ok(sealed) if sealed.rows > 0 => {
                    tracing::info!(
                        rows = sealed.rows,
                        "sealed records into the integrity chain"
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "could not seal the integrity chain"),
            }

            // Shared with the hook, which SQLite calls from inside this same
            // thread — the mutex is for the borrow checker, not contention.
            let touched: Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::new()));
            if sink.is_some() {
                let collect = Arc::clone(&touched);
                // A failure here means no live updates, not a broken writer:
                // capture must keep storing either way.
                let installed = conn.update_hook(Some(
                    move |action: Action, _db: &str, table: &str, rowid: i64| {
                        if table != "tool_call" || action == Action::SQLITE_DELETE {
                            return;
                        }
                        if let Ok(mut rows) = collect.lock() {
                            rows.push(rowid);
                        }
                    },
                ));
                if let Err(e) = installed {
                    tracing::warn!(error = %e, "no update hook; the live view will not update");
                }
            }

            while let Ok(job) = rx.recv() {
                job(&conn);
                let Some(sink) = &sink else { continue };
                let rows = touched.lock().map(|mut r| std::mem::take(&mut *r));
                match rows {
                    Ok(rows) if !rows.is_empty() => sink(&rows),
                    Ok(_) => {}
                    Err(_) => tracing::warn!("change list poisoned; live updates stop here"),
                }
            }
            tracing::debug!("writer thread finished");
        })?;

    Ok(WriteHandle { tx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_run_in_submission_order_on_one_connection() {
        let handle = spawn(Db::open_in_memory().expect("db")).expect("spawn");

        for i in 0..5 {
            handle
                .submit(move |conn| {
                    conn.execute(
                        "INSERT INTO raw_event (lane, source_ref, content_sha256, ingested_at, body)
                         VALUES ('transcript', 'w', ?1, 0, ?1)",
                        [i.to_string()],
                    )
                    .expect("insert");
                })
                .expect("submit");
        }

        let order: Vec<String> = handle
            .submit_blocking(|conn| {
                let mut stmt = conn
                    .prepare("SELECT body FROM raw_event ORDER BY id")
                    .expect("prepare");
                stmt.query_map([], |r| r.get::<_, String>(0))
                    .expect("query")
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .expect("rows")
            })
            .expect("read back");

        assert_eq!(order, ["0", "1", "2", "3", "4"], "FIFO, single writer");
    }

    #[test]
    fn a_full_queue_is_refused_rather_than_buffered() {
        let handle = spawn_with_depth(Db::open_in_memory().expect("db"), 1).expect("spawn");
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();

        // Occupy the writer so nothing drains.
        handle
            .submit(move |_| {
                let _ = release_rx.recv();
            })
            .expect("submit blocker");

        // Fill the queue, then prove the next one is refused rather than queued.
        let mut refused = None;
        for _ in 0..64 {
            if let Err(e) = handle.try_submit(|_| {}) {
                refused = Some(e);
                break;
            }
        }
        assert_eq!(
            refused,
            Some(SubmitError::Full),
            "a bounded queue must refuse, not grow"
        );

        let _ = release_tx.send(());
    }

    #[test]
    fn the_writer_outlives_a_dropped_clone() {
        let handle = spawn(Db::open_in_memory().expect("db")).expect("spawn");
        let clone = handle.clone();
        drop(handle);
        assert!(
            clone.submit(|_| {}).is_ok(),
            "one dropped handle is not shutdown"
        );
    }

    /// The live channel of task 6.9, at its narrowest: a committed write to
    /// `tool_call` is reported, and nothing else is.
    #[test]
    fn a_committed_tool_call_is_reported_and_other_tables_are_not() {
        use crate::model::TranscriptFacts;

        let (tx, rx) = std::sync::mpsc::channel::<Vec<i64>>();
        let sink: ChangeSink = Arc::new(move |rows: &[i64]| {
            let _ = tx.send(rows.to_vec());
        });
        let handle = spawn_watching(Db::open_in_memory().expect("db"), sink).expect("spawn");

        // A write to another table says nothing.
        handle
            .submit_blocking(|conn| {
                conn.execute(
                    "INSERT INTO raw_event (lane, source_ref, content_sha256, ingested_at, body)
                     VALUES ('transcript', 'w', 'h', 0, '{}')",
                    [],
                )
                .expect("insert");
            })
            .expect("submit");

        // Then one to `tool_call`, which does.
        handle
            .submit_blocking(|conn| {
                crate::project::upsert_transcript(
                    conn,
                    "toolu_1",
                    &TranscriptFacts {
                        tool_name: Some("Bash".into()),
                        called_at: Some(1),
                        ..TranscriptFacts::default()
                    },
                )
                .expect("upsert");
            })
            .expect("submit");

        let reported = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the tool_call write");
        assert_eq!(reported.len(), 1, "one row, once");
        assert!(
            rx.try_recv().is_err(),
            "the raw_event write reported nothing"
        );
    }

    /// The second lane completing a row is an update, and the live view needs
    /// it: that is where the duration and the decision come from.
    #[test]
    fn completing_a_row_is_reported_as_well_as_creating_it() {
        use crate::model::{OtelFacts, TranscriptFacts};

        let (tx, rx) = std::sync::mpsc::channel::<Vec<i64>>();
        let sink: ChangeSink = Arc::new(move |rows: &[i64]| {
            let _ = tx.send(rows.to_vec());
        });
        let handle = spawn_watching(Db::open_in_memory().expect("db"), sink).expect("spawn");

        handle
            .submit_blocking(|conn| {
                crate::project::upsert_transcript(
                    conn,
                    "toolu_1",
                    &TranscriptFacts {
                        tool_name: Some("Bash".into()),
                        called_at: Some(1),
                        ..TranscriptFacts::default()
                    },
                )
                .expect("upsert");
            })
            .expect("submit");
        let created = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("created");

        handle
            .submit_blocking(|conn| {
                crate::project::upsert_otel(
                    conn,
                    "toolu_1",
                    &OtelFacts {
                        duration_ms: Some(900),
                        decision: Some("accept".into()),
                        ..OtelFacts::default()
                    },
                )
                .expect("upsert");
            })
            .expect("submit");
        let completed = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("completed");

        assert_eq!(created, completed, "the same row, reported again");
    }

    /// A dead writer must be reported, not silently swallowed: capture has
    /// stopped, and the tray and `doctor` need to be able to say so.
    #[test]
    fn a_writer_that_died_is_reported_rather_than_swallowed() {
        let handle = spawn(Db::open_in_memory().expect("db")).expect("spawn");

        // Kill the thread the way a bug would.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let _ = handle.submit_blocking(|_| panic!("writer bug"));
        std::panic::set_hook(previous);

        // Give the thread a moment to unwind and drop the receiver.
        for _ in 0..100 {
            if handle.submit(|_| {}) == Err(SubmitError::Stopped) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("a dead writer should report SubmitError::Stopped");
    }
}
