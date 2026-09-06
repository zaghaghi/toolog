//! The background examination: a backfill that can be paused, and a queue for
//! calls as they arrive (tasks 13.7, 13.8).
//!
//! # Why this is not a tab activation
//!
//! 3,618 calls at the measured 1.07 seconds each is **65 minutes**. Phase 11
//! exists because a 2.3-second tab activation was intolerable; a 65-minute one
//! is not a thing to hide behind a spinner. So this runs oldest-first in its own
//! thread, reports progress, survives the window being closed, and stops when
//! the model is unset.
//!
//! # Where the work comes from
//!
//! Not from an in-memory queue. Each batch asks the store which calls this
//! (model, prompt) pair has no verdict for — [`toolog_core::llm::pending`] —
//! which makes the store itself the queue. Three properties fall out of that
//! and none of them would from a `VecDeque`: it survives a restart, it cannot
//! drift from what was actually recorded, and a call analysed by the live path
//! disappears from the backfill without the two having to coordinate.
//!
//! # The live path
//!
//! A call arriving is offered to the same worker behind the same switch. It
//! goes through a small bounded queue because inference is not a millisecond
//! and the ingestion thread must not wait for it — and when that queue is full
//! the call is **dropped rather than queued**, because the backfill will reach
//! it anyway. Losing a place in line costs nothing; blocking ingestion costs
//! the capture this tool exists to perform.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};

use toolog_core::llm::{self, Pair, Record};
use toolog_core::writer::WriteHandle;
use toolog_core::{Db, raw};
use toolog_llm::engine::Engine;

use crate::model::AnalysisStatus;

/// How many pending calls one batch reads from the store.
///
/// Large enough that the query's cost disappears against 32 seconds of
/// inference, small enough that unsetting the model is felt within a call or
/// two rather than at the end of a long slice.
const BATCH: u32 = 32;

/// How many verdicts are written in one transaction.
///
/// One write per verdict would be 3,618 transactions; one write at the end
/// would lose an hour of work to a crash. A batch is the middle, and it is the
/// same batch the reader used, so the two cannot drift.
const WRITE_EVERY: usize = 8;

/// How many arriving calls may wait for the model before they are dropped.
const LIVE_QUEUE: usize = 16;

/// A running examination.
///
/// Holds the engine handle and the two threads' switches. Dropping it does not
/// stop anything; [`Analysis::stop`] does, and the resident process calls it on
/// shutdown and whenever the model changes.
pub struct Analysis {
    engine: Engine,
    pair: Pair,
    model_path: PathBuf,
    paused: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    done: Arc<AtomicI64>,
    failed: Arc<AtomicI64>,
    skipped: Arc<AtomicU64>,
    last_error: Arc<Mutex<Option<String>>>,
    live: SyncSender<LiveJob>,
}

impl std::fmt::Debug for Analysis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analysis")
            .field("model", &self.model_path)
            .field("pair", &self.pair.short())
            .finish_non_exhaustive()
    }
}

/// One arriving call, offered to the model.
struct LiveJob {
    tool_use_id: String,
    command: String,
}

impl Analysis {
    /// Load the model, start both threads, and begin working through the store.
    ///
    /// `db_path` rather than a connection: each thread opens its own reader, so
    /// neither contends with the window's under WAL.
    pub fn start(
        db_path: PathBuf,
        model_path: PathBuf,
        model_fingerprint: &str,
        writer: WriteHandle,
    ) -> anyhow::Result<Self> {
        let prompt = toolog_llm::Prompt::current();
        let pair = Pair::new(model_fingerprint, prompt.fingerprint().to_string());
        let engine = Engine::start(&model_path, model_fingerprint, prompt)?;

        let paused = Arc::new(AtomicBool::new(false));
        let stopping = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicI64::new(0));
        let failed = Arc::new(AtomicI64::new(0));
        let skipped = Arc::new(AtomicU64::new(0));
        let last_error = Arc::new(Mutex::new(None));
        let (live_tx, live_rx) = sync_channel::<LiveJob>(LIVE_QUEUE);

        let this = Self {
            engine,
            pair,
            model_path,
            paused,
            stopping,
            done,
            failed,
            skipped,
            last_error,
            live: live_tx,
        };

        this.spawn_live(writer.clone(), live_rx)?;
        this.spawn_backfill(db_path, writer)?;
        Ok(this)
    }

    /// Which model and prompt these verdicts answer for.
    #[must_use]
    pub fn pair(&self) -> &Pair {
        &self.pair
    }

    /// What was loaded, for the Status card.
    #[must_use]
    pub fn loaded(&self) -> &toolog_llm::engine::LoadedModel {
        self.engine.loaded()
    }

    /// Stop the backfill without unloading the model.
    ///
    /// The live path keeps running: it is nearly free, and a user who paused a
    /// 65-minute backfill has not asked to stop looking at what happens next.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    /// Put the model down and let both threads finish.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.engine.stop();
    }

    /// Offer an arriving call to the model (task 13.8).
    ///
    /// Never blocks. A full queue means the model is behind, and the backfill
    /// will reach this call in its own time.
    pub fn observe(&self, tool_use_id: &str, command: &str) {
        if self.stopping.load(Ordering::SeqCst) {
            return;
        }
        let job = LiveJob {
            tool_use_id: tool_use_id.to_string(),
            command: command.to_string(),
        };
        // A full queue means the model is behind and the backfill will get
        // there; a disconnected one means it has stopped. Neither is worth
        // waiting for, and only the first is worth counting.
        if let Err(TrySendError::Full(_)) = self.live.try_send(job) {
            self.skipped.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[must_use]
    pub fn status(&self) -> AnalysisStatus {
        AnalysisStatus {
            running: !self.stopping.load(Ordering::SeqCst),
            paused: self.paused.load(Ordering::SeqCst),
            done_this_run: self.done.load(Ordering::SeqCst),
            failed_this_run: self.failed.load(Ordering::SeqCst),
            skipped_live: i64::try_from(self.skipped.load(Ordering::Relaxed)).unwrap_or(i64::MAX),
            last_error: self.last_error.lock().ok().and_then(|e| e.clone()),
        }
    }

    /// The thread that works backwards through history.
    fn spawn_backfill(&self, db_path: PathBuf, writer: WriteHandle) -> anyhow::Result<()> {
        let ctx = self.context();
        std::thread::Builder::new()
            .name("toolog-llm-backfill".into())
            .spawn(move || {
                let db = match Db::open(&db_path) {
                    Ok(db) => db,
                    Err(e) => {
                        ctx.note_error(format!("the examination could not open the store: {e}"));
                        return;
                    }
                };
                ctx.backfill_loop(&db, &writer);
            })?;
        Ok(())
    }

    /// The thread that takes arriving calls.
    ///
    /// It opens no read connection of its own: the command is already in hand
    /// from the live sink, and the verdict goes to the writer. The backfill is
    /// the only half that has to ask the store anything.
    fn spawn_live(&self, writer: WriteHandle, rx: Receiver<LiveJob>) -> anyhow::Result<()> {
        let ctx = self.context();
        std::thread::Builder::new()
            .name("toolog-llm-live".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    if ctx.stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    let record = ctx.examine(&job.tool_use_id, &job.command);
                    ctx.write(&writer, vec![record]);
                }
            })?;
        Ok(())
    }

    /// The state both threads share. Not the `Analysis` itself, which owns the
    /// live sender and must not be kept alive by a thread that never sends.
    fn context(&self) -> Worker {
        Worker {
            engine: self.engine.clone(),
            pair: self.pair.clone(),
            paused: Arc::clone(&self.paused),
            stopping: Arc::clone(&self.stopping),
            done: Arc::clone(&self.done),
            failed: Arc::clone(&self.failed),
            last_error: Arc::clone(&self.last_error),
        }
    }
}

/// What a thread needs to examine a call and record the answer.
#[derive(Clone)]
struct Worker {
    engine: Engine,
    pair: Pair,
    paused: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    done: Arc<AtomicI64>,
    failed: Arc<AtomicI64>,
    last_error: Arc<Mutex<Option<String>>>,
}

impl Worker {
    /// One command in, one row of `llm_verdict` out — including the failure
    /// rows, which are the point of task 13.10.
    fn examine(&self, tool_use_id: &str, command: &str) -> Record {
        let at = raw::now_ms();
        match self.engine.analyze(command) {
            Ok(answer) => match answer.verdict {
                Ok(v) => {
                    self.done.fetch_add(1, Ordering::SeqCst);
                    Record::ok(tool_use_id, v, at, answer.ms)
                }
                Err(why) => {
                    self.failed.fetch_add(1, Ordering::SeqCst);
                    Record::failed(tool_use_id, why, at, answer.ms)
                }
            },
            // The model itself failed — a context overflow, a worker that has
            // gone. Also a verdict, and also recorded: "asked and could not
            // answer" is a fact, and one that stops this call being retried on
            // every pass forever.
            Err(e) => {
                self.failed.fetch_add(1, Ordering::SeqCst);
                Record::failed(tool_use_id, e.to_string(), at, 0)
            }
        }
    }

    /// Hand verdicts to the process's one writer (task 13.6).
    fn write(&self, writer: &WriteHandle, records: Vec<Record>) {
        if records.is_empty() {
            return;
        }
        let pair = self.pair.clone();
        let outcome = writer.submit_blocking(move |conn| llm::record(conn, &pair, &records));
        match outcome {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => self.note_error(format!("verdicts not written: {e}")),
            Err(e) => self.note_error(format!("the writer has stopped: {e}")),
        }
    }

    fn note_error(&self, message: String) {
        tracing::warn!(error = %message, "analysis");
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = Some(message);
        }
    }

    /// Read a batch, examine it, write it, repeat until there is nothing left.
    ///
    /// The pause is checked between calls rather than between batches, so
    /// pausing costs at most one inference rather than up to 32.
    fn backfill_loop(&self, db: &Db, writer: &WriteHandle) {
        loop {
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            if self.paused.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(250));
                continue;
            }

            let batch = match llm::pending(db.conn(), &self.pair, BATCH) {
                Ok(batch) => batch,
                Err(e) => {
                    self.note_error(format!("the examination could not read the store: {e}"));
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    continue;
                }
            };

            if batch.is_empty() {
                // Caught up. New calls arrive on the live path; this wakes
                // periodically because a `toolog backfill` in another process
                // can add history without telling anyone.
                std::thread::sleep(std::time::Duration::from_secs(30));
                continue;
            }

            let mut pending_writes = Vec::with_capacity(WRITE_EVERY);
            for call in batch {
                if self.stopping.load(Ordering::SeqCst) || self.paused.load(Ordering::SeqCst) {
                    break;
                }
                pending_writes.push(self.examine(&call.tool_use_id, &call.command));
                if pending_writes.len() >= WRITE_EVERY {
                    self.write(writer, std::mem::take(&mut pending_writes));
                }
            }
            // Whatever the batch ended with, including what a pause interrupted:
            // work already done is never thrown away.
            self.write(writer, pending_writes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_with_nothing_done_says_so_rather_than_looking_finished() {
        let status = AnalysisStatus {
            running: true,
            paused: false,
            done_this_run: 0,
            failed_this_run: 0,
            skipped_live: 0,
            last_error: None,
        };
        assert!(status.running);
        assert_eq!(status.done_this_run, 0);
    }

    /// The batch sizes have to relate to each other or the write batching does
    /// nothing: a write batch larger than a read batch would only ever flush at
    /// the end of the loop.
    #[test]
    fn a_read_batch_holds_several_write_batches() {
        assert!(BATCH as usize > WRITE_EVERY);
        assert!(
            (BATCH as usize).is_multiple_of(WRITE_EVERY),
            "so no batch ends ragged"
        );
    }
}
