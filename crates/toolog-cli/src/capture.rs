//! The resident capture supervisor: one writer, two lanes ([ADR-0007]).
//!
//! Owns the OTLP receiver, the transcript tailer and the process's single
//! database write handle, and it is the thing the tray switches on and off.
//! Deliberately free of Tauri so the whole lifecycle — start, pause, resume,
//! shut down — is testable without a window.
//!
//! # Pausing is not symmetric between the lanes, and says so
//!
//! Transcripts are on disk: pausing skips them and a later pass resumes from
//! the stored byte offset, so nothing is lost. OTEL events are not replayable,
//! so anything exported while paused is **gone**. Resuming therefore triggers a
//! catch-up scan of the transcripts, and the discarded OTLP batches are counted
//! rather than forgotten.
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use toolog_core::model::ToolCall;
use toolog_core::writer::{self, WriteHandle};
use toolog_core::{Db, query};
use toolog_ingest::projector::TranscriptProjector;
use toolog_ingest::{backfill, discover, tail};
use toolog_otlp::server::{Collector, CollectorHandle, CounterSnapshot};

/// How often the live view is polled for new rows.
const LIVE_INTERVAL: Duration = Duration::from_millis(500);

/// Most rows to emit in one live tick, so a backfill cannot flood the UI.
const LIVE_BATCH: u32 = 200;

/// Somewhere to send tool calls as they land.
pub type LiveSink = Arc<dyn Fn(&ToolCall) + Send + Sync>;

/// What the tray and `collector_status` report.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Status {
    /// The address the receiver actually bound, which may not be the preferred
    /// one.
    pub endpoint: String,
    pub listening: bool,
    pub paused: bool,
    pub counters: CounterSnapshot,
    /// Records stored since local midnight.
    pub events_today: i64,
    pub tool_calls: i64,
    pub database: PathBuf,
    /// True when the receiver had to fall back from the preferred port, which
    /// means Claude Code's configuration needs rewriting to match.
    pub port_changed: bool,
}

/// The running capture pipeline.
pub struct Capture {
    db_path: PathBuf,
    writer: WriteHandle,
    collector: CollectorHandle,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
    preferred: SocketAddr,
}

impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture")
            .field("db_path", &self.db_path)
            .field("endpoint", &self.collector.endpoint())
            .field("paused", &self.is_paused())
            .finish_non_exhaustive()
    }
}

impl Capture {
    /// Open the database, bind the receiver and start watching transcripts.
    ///
    /// `transcripts` is the directory to watch; `None` uses
    /// `~/.claude/projects`, and a missing directory is not fatal — the OTLP
    /// lane still works, and `doctor` reports the absence.
    pub async fn start(
        db_path: PathBuf,
        preferred: SocketAddr,
        transcripts: Option<PathBuf>,
        live: Option<LiveSink>,
    ) -> anyhow::Result<Self> {
        let addr = toolog_otlp::port::choose(&preferred.ip().to_string(), preferred.port())?;
        let writer = writer::spawn(Db::open(&db_path)?)?;
        let paused = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let collector =
            Collector::start_with_writer(writer.clone(), addr, Arc::clone(&paused)).await?;

        let mut threads = Vec::new();
        if let Some(root) = transcripts.or_else(discover::projects_dir) {
            threads.push(spawn_tailer(
                root,
                writer.clone(),
                Arc::clone(&paused),
                Arc::clone(&stop),
            ));
        } else {
            tracing::warn!("no transcript directory to watch; running on the OTLP lane alone");
        }

        if let Some(sink) = live {
            threads.push(spawn_live(&db_path, sink, Arc::clone(&stop))?);
        }

        Ok(Self {
            db_path,
            writer,
            collector,
            paused,
            stop,
            threads,
            preferred,
        })
    }

    /// The endpoint to write into Claude Code's configuration.
    #[must_use]
    pub fn endpoint(&self) -> String {
        self.collector.endpoint()
    }

    /// Whether the receiver had to move off the preferred port.
    ///
    /// When it did, the `settings.json` endpoint no longer matches and capture
    /// will silently stop until it is rewritten — so this is surfaced rather
    /// than logged.
    #[must_use]
    pub fn port_changed(&self) -> bool {
        self.collector.addr() != self.preferred
    }

    /// The shared write handle, for backfills and other on-demand work.
    #[must_use]
    pub fn writer(&self) -> &WriteHandle {
        &self.writer
    }

    /// A separate read connection. WAL lets it read while the writer writes.
    pub fn reader(&self) -> toolog_core::Result<Db> {
        Db::open(&self.db_path)
    }

    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Stop storing anything, without unbinding the port.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
        tracing::info!("capture paused");
    }

    /// Resume, and catch up on whatever the transcripts recorded meanwhile.
    ///
    /// The catch-up is what makes pausing cheap for the transcript lane: byte
    /// offsets are stored per file, so a scan picks up exactly what was missed.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
        self.catch_up();
        tracing::info!("capture resumed");
    }

    /// Re-scan the transcript directory once, off the writer thread.
    pub fn catch_up(&self) {
        let root = discover::projects_dir();
        let _ = self.writer.submit(move |conn| {
            let Some(root) = root else { return };
            let mut projector = TranscriptProjector::new();
            for path in discover::transcripts(&root) {
                if let Err(e) = backfill::ingest_and_project(conn, &path, &mut projector) {
                    tracing::warn!(path = %path.display(), error = %e, "catch-up ingest failed");
                }
            }
        });
    }

    /// A snapshot for the tray and the UI.
    pub fn status(&self) -> toolog_core::Result<Status> {
        let reader = self.reader()?;
        Ok(Status {
            endpoint: self.endpoint(),
            listening: true,
            paused: self.is_paused(),
            counters: self.collector.counters(),
            events_today: query::events_since(reader.conn(), start_of_today_ms())?,
            tool_calls: query::stats_totals(reader.conn())?.tool_calls,
            database: self.db_path.clone(),
            port_changed: self.port_changed(),
        })
    }

    /// Stop both lanes and wait for their threads.
    pub fn shutdown(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.collector.shutdown();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        tracing::info!("capture stopped");
    }
}

/// Watch transcripts, submitting each settled burst to the shared writer.
fn spawn_tailer(
    root: PathBuf,
    writer: WriteHandle,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        // One projector across the whole run: subagent attribution resolves
        // across files, so it must not be rebuilt per burst.
        let projector = Arc::new(Mutex::new(TranscriptProjector::new()));
        let tail = tail::Tail::new(&root);

        let result = tail.watch(
            |paths| {
                if paused.load(Ordering::Relaxed) {
                    return;
                }
                let paths = paths.to_vec();
                let projector = Arc::clone(&projector);
                // Blocking submit: the transcript is on disk, so waiting for
                // the writer costs nothing and loses nothing.
                let _ = writer.submit(move |conn| {
                    let mut projector = match projector.lock() {
                        Ok(p) => p,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    for path in &paths {
                        match backfill::ingest_and_project(conn, path, &mut projector) {
                            Ok(report) if report.stored > 0 => tracing::debug!(
                                path = %path.display(),
                                stored = report.stored,
                                "transcript ingested"
                            ),
                            Ok(_) => {}
                            Err(e) => tracing::warn!(
                                path = %path.display(), error = %e, "transcript ingest failed"
                            ),
                        }
                    }
                });
            },
            &|| stop.load(Ordering::Relaxed),
        );

        if let Err(e) = result {
            tracing::error!(error = %e, root = %root.display(), "transcript watch stopped");
        }
    })
}

/// Poll for newly stored tool calls and hand them to the UI.
///
/// A poll rather than a push because both lanes create rows, from different
/// threads, and one cursor over the table covers both without threading a
/// callback through each. Phase 6 replaces it with a real channel.
fn spawn_live(
    db_path: &Path,
    sink: LiveSink,
    stop: Arc<AtomicBool>,
) -> anyhow::Result<std::thread::JoinHandle<()>> {
    let db = Db::open(db_path)?;
    let mut cursor = query::max_tool_call_rowid(db.conn())?;

    Ok(std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(LIVE_INTERVAL);
            match query::tool_calls_after_rowid(db.conn(), cursor, LIVE_BATCH) {
                Ok(rows) => {
                    for (rowid, call) in rows {
                        cursor = cursor.max(rowid);
                        sink(&call);
                    }
                }
                Err(e) => tracing::warn!(error = %e, "live poll failed"),
            }
        }
    }))
}

/// Local midnight, in milliseconds since the epoch.
fn start_of_today_ms() -> i64 {
    jiff::Zoned::now()
        .start_of_day()
        .map(|z| z.timestamp().as_millisecond())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn ephemeral() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_started_capture_listens_and_reports_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        let capture = Capture::start(
            dir.path().join("t.db"),
            ephemeral(),
            Some(dir.path().join("projects")),
            None,
        )
        .await
        .expect("start");

        let status = capture.status().expect("status");
        assert!(status.listening);
        assert!(!status.paused);
        assert!(status.endpoint.starts_with("http://127.0.0.1:"));
        assert_eq!(status.tool_calls, 0);

        let addr: SocketAddr = status
            .endpoint
            .trim_start_matches("http://")
            .parse()
            .expect("addr");
        assert!(
            toolog_otlp::health::probe(addr).is_up(),
            "the receiver must actually answer, not merely hold the port"
        );

        capture.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pausing_and_resuming_flows_through_to_the_receiver() {
        let dir = tempfile::tempdir().expect("tempdir");
        let capture = Capture::start(
            dir.path().join("t.db"),
            ephemeral(),
            Some(dir.path().join("projects")),
            None,
        )
        .await
        .expect("start");

        capture.pause();
        assert!(capture.is_paused());
        assert!(capture.status().expect("status").paused);

        capture.resume();
        assert!(!capture.is_paused());
        capture.shutdown();
    }

    /// The tailer must survive a transcript directory that is not there:
    /// the OTLP lane is still worth running on its own.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_missing_transcript_directory_is_not_fatal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let capture = Capture::start(
            dir.path().join("t.db"),
            ephemeral(),
            Some(dir.path().join("nowhere")),
            None,
        )
        .await
        .expect("start");
        assert!(capture.status().expect("status").listening);
        capture.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_live_sink_receives_calls_as_they_are_stored() {
        use toolog_core::model::TranscriptFacts;

        let dir = tempfile::tempdir().expect("tempdir");
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_target = Arc::clone(&seen);

        let capture = Capture::start(
            dir.path().join("t.db"),
            ephemeral(),
            Some(dir.path().join("projects")),
            Some(Arc::new(move |call: &ToolCall| {
                if let Ok(mut v) = sink_target.lock() {
                    v.push(call.tool_use_id.clone());
                }
            })),
        )
        .await
        .expect("start");

        capture
            .writer()
            .submit_blocking(|conn| {
                toolog_core::project::upsert_transcript(
                    conn,
                    "toolu_live",
                    &TranscriptFacts {
                        tool_name: Some("Bash".to_string()),
                        ..TranscriptFacts::default()
                    },
                )
            })
            .expect("submit")
            .expect("upsert");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if seen.lock().is_ok_and(|v| !v.is_empty()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        assert_eq!(
            seen.lock().expect("lock").as_slice(),
            ["toolu_live"],
            "a stored call must reach the live view"
        );
        capture.shutdown();
    }
}
