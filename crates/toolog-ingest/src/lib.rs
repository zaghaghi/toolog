//! The transcript lane: the *content* half of the audit trail ([ADR-0002]).
//!
//! Reads `~/.claude/projects/**/*.jsonl` for the untruncated `tool_input` and
//! `toolUseResult` that OTEL cannot carry — OTEL truncates tool inputs at 512
//! characters, and a truncated shell command is not evidence.
//!
//! [`backfill`] and [`tail`] share one parser and one [`projector`], so history
//! and live capture cannot drift apart, and re-projection from `raw_event`
//! produces the same rows as the original ingest.
//!
//! [ADR-0002]: ../../../docs/adr/0002-dual-ingestion-transcripts-and-otel.md

pub mod backfill;
pub mod discover;
pub mod envelope;
pub mod jsonl;
pub mod projector;
pub mod tail;

/// Re-exported from `toolog-core`, where both lanes share it.
pub use toolog_core::normalize;

pub use backfill::{Backfill, BackfillReport};
pub use projector::{ProjectStats, TranscriptProjector};
