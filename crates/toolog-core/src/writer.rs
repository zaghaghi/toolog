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
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

use std::sync::mpsc::{SyncSender, TrySendError};

use rusqlite::Connection;

use crate::Db;

/// Work for the writer thread.
type Job = Box<dyn FnOnce(&Connection) + Send + 'static>;

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
    let (tx, rx) = std::sync::mpsc::sync_channel::<Job>(depth);

    std::thread::Builder::new()
        .name("toolog-writer".into())
        .spawn(move || {
            let conn = db.into_connection();
            while let Ok(job) = rx.recv() {
                job(&conn);
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
