//! Command implementations: `doctor`, `backfill`, `verify`, `export`.
//!
//! A library, not a binary. [ADR-0007] ships one artifact, so `toolog-app`
//! provides the single `toolog` executable and dispatches these by argv.
//!
//! - `doctor` — the install experience (Phase 4). Reports the state of the
//!   Claude Code integration; mutates only under `--fix`.
//! - `backfill` — import existing history (Phase 2).
//! - `verify` — reconcile the two ingestion lanes (Phase 7). Divergence is a
//!   finding: OTEL-only calls were rejected, transcript-only calls are gaps in
//!   collection.
//! - `export` — evidence bundles (Phase 5).
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md
