//! Importing existing history, and the shared ingest step the tail reuses.
//!
//! Resumable and safe to re-run: offsets are stored per file, and content-hash
//! deduplication makes a repeat pass a no-op. In the planning corpus 18% of
//! lines were already held, so this is load-bearing rather than theoretical.

use std::path::{Path, PathBuf};

use toolog_core::project::Projector;
use toolog_core::{Connection, Result, raw};

use crate::discover;
use crate::jsonl;
use crate::projector::{ProjectStats, TranscriptProjector, store_line};

/// What one ingest pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileReport {
    pub path: PathBuf,
    /// Complete lines read.
    pub lines: usize,
    /// Lines not already held by content hash.
    pub stored: usize,
    /// Offset to resume from next time.
    pub next_offset: i64,
    /// The file ended mid-line — normal when Claude Code is writing to it.
    pub trailing_partial: bool,
}

/// What a whole backfill did.
#[derive(Debug, Clone, Default)]
pub struct BackfillReport {
    pub files: usize,
    pub lines: usize,
    pub stored: usize,
    pub duplicates: usize,
    pub stats: ProjectStats,
}

/// Called after each file is ingested.
type ProgressFn<'a> = Box<dyn Fn(&FileReport) + 'a>;

/// Import transcripts into the database.
pub struct Backfill<'a> {
    conn: &'a Connection,
    progress: Option<ProgressFn<'a>>,
}

impl std::fmt::Debug for Backfill<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backfill")
            .field("progress", &self.progress.as_ref().map(|_| "<callback>"))
            .finish()
    }
}

impl<'a> Backfill<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self {
            conn,
            progress: None,
        }
    }

    /// Report each file as it completes.
    #[must_use]
    pub fn on_progress(mut self, f: impl Fn(&FileReport) + 'a) -> Self {
        self.progress = Some(Box::new(f));
        self
    }

    /// Ingest every transcript under `~/.claude/projects`.
    pub fn run_default(&self) -> Result<BackfillReport> {
        let Some(root) = discover::projects_dir() else {
            return Ok(BackfillReport::default());
        };
        self.run(&root)
    }

    /// Ingest every transcript under `root`.
    ///
    /// One projector spans every file, which is what lets subagent attribution
    /// resolve across them: an `Agent` result can name a subagent whose calls
    /// were recorded in a file read earlier or later, and `finish()` settles
    /// those once the whole run is in hand.
    ///
    /// **It does not re-project.** A re-projection clears every projection
    /// table, and this crate can only rebuild the transcript half — so doing it
    /// here would delete the OTLP lane's decisions, durations and costs on
    /// every import. Rebuilding from evidence is a deliberate, all-lanes
    /// operation ([`project::Chain`]).
    pub fn run(&self, root: &Path) -> Result<BackfillReport> {
        let mut report = BackfillReport::default();
        let mut projector = TranscriptProjector::new();

        for path in discover::transcripts(root) {
            let file = ingest_and_project(self.conn, &path, &mut projector)?;
            report.files += 1;
            report.lines += file.lines;
            report.stored += file.stored;
            report.duplicates += file.lines - file.stored;
            if let Some(f) = &self.progress {
                f(&file);
            }
        }

        // Anything written before the integrity chain existed is linked in
        // here, so a store imported by the CLI is sealed without needing the
        // application to have been run (task 7.6).
        toolog_core::chain::seal(self.conn)?;

        report.stats = projector.stats().clone();
        Ok(report)
    }
}

/// Read one transcript from `from` (or its stored offset) and store the lines.
///
/// Handles the two ways a file can stop being what we last saw:
///
/// - **Truncated or replaced.** If the file is now shorter than our offset, the
///   offset is meaningless, so it rescans from zero. Deduplication discards what
///   is already held, which is exactly why that is affordable.
/// - **Still being written.** A trailing fragment is never stored, and the
///   offset stops before it.
pub fn ingest_file(conn: &Connection, path: &Path, from: Option<i64>) -> Result<FileReport> {
    let source_ref = path.to_string_lossy().to_string();

    let stored_offset = match from {
        Some(o) => o,
        None => raw::max_offset(conn, &source_ref)?.unwrap_or(0),
    };

    let file = std::fs::File::open(path)?;
    let len = i64::try_from(file.metadata()?.len()).unwrap_or(i64::MAX);
    let start = if stored_offset > len {
        tracing::debug!(
            path = %source_ref,
            stored_offset,
            len,
            "file is shorter than the last offset; rescanning from zero"
        );
        0
    } else {
        stored_offset
    };

    let tx = conn.unchecked_transaction()?;
    let mut report = FileReport {
        path: path.to_path_buf(),
        ..FileReport::default()
    };
    let mut failed = None;

    let outcome = jsonl::read_from(file, start, &mut |line| {
        if failed.is_some() {
            return;
        }
        report.lines += 1;
        match store_line(&tx, &source_ref, line.offset, &line.text) {
            Ok(true) => report.stored += 1,
            Ok(false) => {}
            Err(e) => failed = Some(e),
        }
    })?;

    if let Some(e) = failed {
        return Err(e);
    }
    tx.commit()?;

    report.next_offset = outcome.next_offset;
    report.trailing_partial = outcome.trailing_partial;
    Ok(report)
}

/// Ingest one file and project only the newly stored records.
///
/// The incremental path used by the live tail. Backfill re-projects everything
/// instead, which is cheaper in bulk and lets cross-file attribution resolve.
pub fn ingest_and_project(
    conn: &Connection,
    path: &Path,
    projector: &mut TranscriptProjector,
) -> Result<FileReport> {
    let before = raw::max_id(conn)?;
    let report = ingest_file(conn, path, None)?;

    let tx = conn.unchecked_transaction()?;
    let mut failed = None;
    raw::scan_after(&tx, before, &mut |row| {
        if failed.is_some() {
            return;
        }
        if let Err(e) = projector.project(&tx, row) {
            failed = Some(e);
        }
    })?;
    if let Some(e) = failed {
        return Err(e);
    }
    projector.finish(&tx)?;
    tx.commit()?;

    Ok(report)
}
