//! Watching `~/.claude/projects` and ingesting as Claude Code writes.
//!
//! Read-only: a filesystem watch and nothing on Claude Code's execution path
//! ([ADR-0002]).
//!
//! Two realities shape the design. Writes arrive in **bursts** — a single tool
//! call produces several records in quick succession — so events are coalesced
//! per file and debounced rather than acted on individually. And a file can stop
//! being what we last saw, by truncation or replacement; recovery is to rescan
//! it from zero and let content-hash deduplication discard the overlap, which is
//! affordable precisely because that deduplication exists.
//!
//! [ADR-0002]: ../../../docs/adr/0002-dual-ingestion-transcripts-and-otel.md

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{Event, RecursiveMode, Watcher};
use toolog_core::{Connection, Result};

use crate::backfill::{FileReport, ingest_and_project};
use crate::discover;
use crate::projector::TranscriptProjector;

/// How long to wait for a burst of writes to settle before ingesting.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(300);

/// A live transcript watcher.
pub struct Tail {
    root: PathBuf,
    debounce: Duration,
}

impl std::fmt::Debug for Tail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tail")
            .field("root", &self.root)
            .field("debounce", &self.debounce)
            .finish()
    }
}

impl Tail {
    /// Watch `root` for transcript changes.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            debounce: DEFAULT_DEBOUNCE,
        }
    }

    /// Watch `~/.claude/projects`.
    pub fn for_default_projects() -> Option<Self> {
        discover::projects_dir().map(Self::new)
    }

    /// Override the debounce window.
    #[must_use]
    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }

    /// Watch until `should_stop` returns true, reporting settled bursts of
    /// changed transcripts.
    ///
    /// This is the watching half on its own, with no database in sight. The
    /// application runs it on a thread of its own and submits the ingest to the
    /// process's single writer ([ADR-0007]); [`Tail::run`] is the same thing
    /// with a connection in hand.
    ///
    /// [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md
    pub fn watch(
        &self,
        mut on_batch: impl FnMut(&[PathBuf]),
        should_stop: &dyn Fn() -> bool,
    ) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                // A send failure only means the receiver is gone, which is how
                // shutdown looks from here.
                let _ = tx.send(event);
            }
        })
        .map_err(|e| watch_error(&e))?;

        watcher
            .watch(&self.root, RecursiveMode::Recursive)
            .map_err(|e| watch_error(&e))?;

        let mut pending: HashSet<PathBuf> = HashSet::new();
        let mut oldest: Option<Instant> = None;

        loop {
            if should_stop() {
                return Ok(());
            }

            match rx.recv_timeout(self.debounce) {
                Ok(event) => {
                    for path in event.paths {
                        if is_transcript(&path) {
                            pending.insert(path);
                            oldest.get_or_insert_with(Instant::now);
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }

            // Report once the burst has settled.
            let settled = oldest.is_some_and(|t| t.elapsed() >= self.debounce);
            if settled && !pending.is_empty() {
                let mut paths: Vec<_> = pending.drain().collect();
                paths.sort();
                oldest = None;
                paths.retain(|p| p.exists());
                if !paths.is_empty() {
                    on_batch(&paths);
                }
            }
        }
    }

    /// Watch until `should_stop` returns true, ingesting as files change.
    ///
    /// `on_file` is called after each file is ingested. Blocks; run it on its
    /// own thread.
    pub fn run(
        &self,
        conn: &Connection,
        mut on_file: impl FnMut(&FileReport),
        should_stop: &dyn Fn() -> bool,
    ) -> Result<()> {
        let mut projector = TranscriptProjector::new();
        self.watch(
            |paths| {
                for path in paths {
                    match ingest_and_project(conn, path, &mut projector) {
                        Ok(report) => on_file(&report),
                        // One unreadable file must not stop the watch; a
                        // transcript can be mid-rotation when we reach it.
                        Err(e) => {
                            tracing::warn!(path = %path.display(), error = %e, "ingest failed");
                        }
                    }
                }
            },
            should_stop,
        )
    }
}

fn is_transcript(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "jsonl")
}

fn watch_error(e: &notify::Error) -> toolog_core::Error {
    toolog_core::Error::Io(std::io::Error::other(e.to_string()))
}
