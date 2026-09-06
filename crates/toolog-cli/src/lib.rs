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
//! - `uninstall` — the reverse of the install, put back rather than deleted
//!   (Phase 8).
//! - `model` — point the local second opinion at a `.gguf` (Phase 13).
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

/// The background examination (Phase 13). Absent from a build without the
/// `inference` feature, which is task 13.19's reserved fallback.
#[cfg(feature = "inference")]
pub mod analysis;
pub mod capture;
pub mod cli;
pub mod commands;
pub mod doctor;
pub mod launchagent;
pub mod logging;
pub mod model;
pub mod prefs;
pub mod settings;
pub mod uninstall;
