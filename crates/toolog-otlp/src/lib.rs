//! The OTLP lane: the *decision* half of the audit trail ([ADR-0002]).
//!
//! Carries what transcripts never record — who approved each call and how, how
//! long it took, what it cost, and the calls that were **rejected**. A denied
//! tool call leaves no transcript trace whatsoever, so this lane is the only
//! place a refusal is ever observed.
//!
//! The receiver is embedded rather than delegated to `otelcol` ([ADR-0005]): the
//! brief asks for a one-artifact install, and Claude Code is the only client,
//! speaking OTLP over loopback HTTP.
//!
//! Records land in `raw_event` before anything interprets them ([ADR-0004]), so
//! an event type this build has never seen is kept and can be projected later.
//!
//! [ADR-0002]: ../../../docs/adr/0002-dual-ingestion-transcripts-and-otel.md
//! [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md
//! [ADR-0005]: ../../../docs/adr/0005-embedded-otlp-receiver.md

pub mod attrs;
pub mod decode;
pub mod events;
pub mod ingest;
pub mod port;
pub mod projector;
pub mod server;
pub mod testing;

pub use ingest::{IngestStats, ingest_records};
pub use projector::{OtlpProjector, OtlpStats};
pub use server::{Collector, CollectorHandle};
