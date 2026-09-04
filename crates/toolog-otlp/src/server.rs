//! The embedded OTLP receiver.
//!
//! An `axum` server on loopback ([ADR-0008]) with three routes:
//!
//! - `POST /v1/logs` — the signal the audit trail is built from, and `POST /`
//!   for a configuration written without the path
//! - `POST /v1/metrics` — accepted and dropped, so a user who enables metrics
//!   gets a clean 204 instead of connection errors in their Claude Code debug log
//! - `GET /healthz` — used by `doctor` and the tray indicator
//!
//! # Why a writer thread
//!
//! SQLite writes are blocking and [ADR-0007] gives the process a single write
//! handle. Handlers therefore decode, hand the batch to [`toolog_core::writer`],
//! and answer immediately; one thread owns the connection and drains the queue.
//!
//! That writer is shared with the transcript tailer, which is the point: two
//! connections would reintroduce exactly the lock contention ADR-0007 removes.
//!
//! The queue bound is the backpressure. If ingestion falls behind, the submit
//! fails and the handler answers 503 rather than growing a queue without limit
//! — and the drop is **counted and surfaced**, because an audit tool that
//! quietly loses records is worse than one that admits it cannot keep up.
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md
//! [ADR-0008]: ../../../docs/adr/0008-local-only-zero-egress.md

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use toolog_core::Db;
use toolog_core::writer::{self, SubmitError, WriteHandle};

use crate::decode::{self, Encoding};
use crate::ingest;

/// Largest export body accepted. Claude Code batches on a short interval, so
/// legitimate bodies are far smaller; this only bounds a pathological one.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Counters the tray and `doctor` report.
#[derive(Debug, Default)]
pub struct Counters {
    pub batches: AtomicU64,
    pub records: AtomicU64,
    /// Batches refused because the writer could not keep up. Surfaced, never
    /// swallowed.
    pub dropped: AtomicU64,
    pub rejected_bodies: AtomicU64,
    /// Batches discarded because the user paused capture. A separate counter
    /// from `dropped`: one is the user's choice, the other is a shortfall.
    pub paused_drops: AtomicU64,
}

impl Counters {
    /// A readable snapshot.
    #[must_use]
    pub fn snapshot(&self) -> CounterSnapshot {
        CounterSnapshot {
            batches: self.batches.load(Ordering::Relaxed),
            records: self.records.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            rejected_bodies: self.rejected_bodies.load(Ordering::Relaxed),
            paused_drops: self.paused_drops.load(Ordering::Relaxed),
        }
    }
}

/// A point-in-time reading of [`Counters`].
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS,
)]
#[ts(export_to = "unused/")]
// A health probe may be reading a build older or newer than its own, so a
// missing counter is a zero rather than a parse failure.
#[serde(default)]
pub struct CounterSnapshot {
    pub batches: u64,
    pub records: u64,
    pub dropped: u64,
    pub rejected_bodies: u64,
    pub paused_drops: u64,
}

/// Shared handler state.
#[derive(Clone)]
struct AppState {
    writer: WriteHandle,
    counters: Arc<Counters>,
    /// Set while the user has paused capture.
    paused: Arc<AtomicBool>,
}

/// A running receiver.
#[derive(Debug)]
pub struct CollectorHandle {
    addr: SocketAddr,
    counters: Arc<Counters>,
    paused: Arc<AtomicBool>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl CollectorHandle {
    /// The address actually bound, which may not be the preferred port.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The endpoint to write into Claude Code's configuration.
    #[must_use]
    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Ingest counters.
    #[must_use]
    pub fn counters(&self) -> CounterSnapshot {
        self.counters.snapshot()
    }

    /// Stop storing records without closing the port.
    ///
    /// The port stays bound deliberately: an exporter posting into a refused
    /// connection logs errors the user never asked for, and a paused receiver
    /// that still answers keeps `doctor` truthful about what is running.
    ///
    /// **Records that arrive while paused are gone.** OTEL events are not
    /// replayable from disk the way transcripts are, so this is a real cost —
    /// counted in `paused_drops` rather than hidden.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Ask the server to stop.
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// The embedded OTLP receiver.
#[derive(Debug)]
pub struct Collector;

impl Collector {
    /// Bind `addr`, owning the database outright.
    ///
    /// Starts a writer of its own. Use [`Collector::start_with_writer`] when
    /// the process already has one to share — which the application does.
    pub async fn start(db: Db, addr: SocketAddr) -> std::io::Result<CollectorHandle> {
        let writer = writer::spawn(db)?;
        Self::start_with_writer(writer, addr, Arc::new(AtomicBool::new(false))).await
    }

    /// Bind `addr`, submitting writes to an existing writer.
    ///
    /// `paused` is shared with the rest of the application so one switch stops
    /// both lanes.
    pub async fn start_with_writer(
        writer: WriteHandle,
        addr: SocketAddr,
        paused: Arc<AtomicBool>,
    ) -> std::io::Result<CollectorHandle> {
        let counters = Arc::new(Counters::default());
        let state = AppState {
            writer,
            counters: Arc::clone(&counters),
            paused: Arc::clone(&paused),
        };
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let server = axum::serve(listener, router(state)).with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            });
            if let Err(e) = server.await {
                tracing::error!(error = %e, "otlp receiver stopped");
            }
        });

        tracing::info!(address = %bound, "otlp receiver listening");
        Ok(CollectorHandle {
            addr: bound,
            counters,
            paused,
            shutdown: Some(shutdown_tx),
        })
    }
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/logs", post(logs))
        // Forgiving fallback. A per-signal OTLP endpoint is used verbatim, so a
        // value written without the path posts here instead — which cost Phase 3
        // an end-to-end test before it was noticed. Nothing else posts to a
        // loopback-only port, so accepting logs here has no downside.
        .route("/", post(logs))
        .route("/v1/metrics", post(metrics))
        .route("/v1/traces", post(metrics))
        .route("/healthz", get(healthz))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

async fn logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    let Some(encoding) = Encoding::from_content_type(content_type) else {
        state
            .counters
            .rejected_bodies
            .fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("unsupported content type: {content_type:?}"),
        );
    };

    let request = match decode::logs(encoding, &body) {
        Ok(r) => r,
        Err(e) => {
            state
                .counters
                .rejected_bodies
                .fetch_add(1, Ordering::Relaxed);
            tracing::warn!(error = %e, "rejected an otlp body");
            return (StatusCode::BAD_REQUEST, e.to_string());
        }
    };

    let records = decode::records(request);
    let n = records.len();
    if n == 0 {
        return (StatusCode::OK, String::new());
    }

    if state.paused.load(Ordering::Relaxed) {
        state.counters.paused_drops.fetch_add(1, Ordering::Relaxed);
        // 200, not an error: the export succeeded, we chose not to keep it.
        return (StatusCode::OK, String::new());
    }

    let seq = state.counters.batches.fetch_add(1, Ordering::Relaxed);
    let source_ref = format!("otlp:batch-{seq}");

    let submitted = state.writer.try_submit(move |conn| {
        match ingest::ingest_records(conn, &source_ref, &records) {
            Ok(ingested) => tracing::debug!(
                received = ingested.received,
                stored = ingested.stored,
                rejections = ingested.projected.rejections,
                "otlp batch ingested"
            ),
            // One bad batch must not take down capture for the rest.
            Err(e) => tracing::error!(error = %e, "otlp batch failed"),
        }
    });

    match submitted {
        Ok(()) => {
            state
                .counters
                .records
                .fetch_add(n as u64, Ordering::Relaxed);
            (StatusCode::OK, String::new())
        }
        // Backpressure: refuse rather than queue without limit, and say so.
        Err(SubmitError::Full) => {
            state.counters.dropped.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(records = n, "otlp queue full; batch refused");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "ingest queue full".to_string(),
            )
        }
        Err(SubmitError::Stopped) => {
            state.counters.dropped.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "collector is shutting down".to_string(),
            )
        }
    }
}

/// Accept and drop.
///
/// A user who turns on metrics should not see connection errors in their debug
/// log for a signal this build does not yet store.
async fn metrics() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "paused": state.paused.load(Ordering::Relaxed),
        "counters": state.counters.snapshot(),
    }))
}
