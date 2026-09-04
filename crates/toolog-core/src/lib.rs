//! Storage core: schema, migrations, normalization and the typed query layer.
//!
//! All SQL in the workspace lives in this crate ([ADR-0003]). Other crates reach
//! data through typed functions, never by writing queries of their own.
//!
//! The central invariant is [ADR-0004]: every input record is persisted verbatim
//! to `raw_event` before it is parsed, and every other table is a re-runnable
//! projection of it. Claude Code's formats drift, and data lost at ingestion
//! cannot be recovered.
//!
//! Implementation lands in Phase 1; this crate is scaffolding until then.
//!
//! [ADR-0003]: ../../../docs/adr/0003-sqlite-as-the-embedded-store.md
//! [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md

pub mod constants;
