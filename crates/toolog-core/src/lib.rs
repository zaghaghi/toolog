//! Storage core: schema, migrations, projection and the typed query layer.
//!
//! All SQL in the workspace lives in this crate ([ADR-0003]). Other crates reach
//! data through typed functions, never by writing queries of their own.
//!
//! # The shape of the thing
//!
//! [`raw`] is the evidence store. Every input record — a transcript line, an
//! OTLP log record — is written there verbatim before anything parses it. The
//! remaining tables are a projection of it, rebuildable at any time via
//! [`project::reproject`]. This is [ADR-0004], and it is the property that makes
//! the tool survive Claude Code changing its formats: one ordinary user's
//! history already spans 12 versions, 21 record types, and three different
//! shapes of `toolUseResult`.
//!
//! [`project`] holds the two lane-specific upserts. They are order-independent
//! and write disjoint column sets ([ADR-0009]), so a call may be created by
//! either lane and completed by the other with the same final row. Rows carry a
//! `provenance` bitmask recording which lanes witnessed them, so a row the
//! transcript never reached is visible as such. A **refusal**, though, is read
//! from the `decision` column and never from provenance: Phase 4 measured a
//! real denial and found it in both lanes ([ADR-0009]).
//!
//! [`verify`] is the completeness layer: which calls carry both lanes, which
//! sessions are missing their approval record, and the windows in which nothing
//! was watching. It is a separate claim from integrity, and the more important
//! one — a record that was never captured leaves nothing to tamper with.
//!
//! [`chain`] is the integrity layer: every stored record is linked to the one
//! before it, so modifying stored evidence is detectable. It is deliberately a
//! separate claim from *completeness*, which reconciliation answers — a record
//! that was never captured leaves nothing to break.
//!
//! [`llm`] is the second opinion (Phase 13): the ledger of what a local model
//! said about the calls no rule matched. It is the one derived-looking thing in
//! this crate that is **stored** rather than recomputed, because an LLM answer
//! is not reproducible and so is not a derivation in ADR-0004's sense — see
//! [ADR-0013]. It is advisory throughout: nothing here feeds [`rules`].
//!
//! [`rules`] is the risk layer: rules written as data, compiled to bound SQL
//! here so a rules file can express a question but never a query. Findings are
//! computed rather than stored, for the same reason the projections are.
//!
//! [ADR-0003]: ../../../docs/adr/0003-sqlite-as-the-embedded-store.md
//! [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md
//! [ADR-0009]: ../../../docs/adr/0009-correlate-on-tool-use-id.md
//! [ADR-0013]: ../../../docs/adr/0013-a-verdict-is-stored-not-recomputed.md

pub mod chain;
pub mod constants;
pub mod db;
pub mod error;
pub mod fts;
pub mod llm;
pub mod migrations;
pub mod model;
pub mod normalize;
pub mod project;
pub mod query;
pub mod raw;
pub mod redact;
pub mod retention;
pub mod rules;
pub mod verify;
pub mod writer;

pub use db::Db;
pub use error::{Error, Result};

/// Re-exported so the lane crates can pass a connection without depending on
/// `rusqlite` themselves — [ADR-0003] keeps SQL inside this crate.
///
/// [ADR-0003]: ../../../docs/adr/0003-sqlite-as-the-embedded-store.md
pub use rusqlite::Connection;
