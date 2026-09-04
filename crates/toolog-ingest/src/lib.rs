//! The transcript lane: the *content* half of the audit trail ([ADR-0002]).
//!
//! Reads `~/.claude/projects/**/*.jsonl` for the untruncated `tool_input` and
//! `toolUseResult` that OTEL cannot carry — OTEL truncates tool inputs at 512
//! characters, and a truncated shell command is not evidence.
//!
//! Backfill and live tailing share one parser so history and live capture
//! cannot drift apart.
//!
//! Two properties the parser must hold, both forced by real data in the
//! planning corpus (39 files, 12 Claude Code versions):
//!
//! - `toolUseResult` appears as an object, a bare string *and* an array.
//!   Assuming one shape fails on roughly 7% of records.
//! - Unknown tools and unknown record types are stored and skipped, never
//!   rejected. New tools ship constantly.
//!
//! Implementation lands in Phase 2.
//!
//! [ADR-0002]: ../../../docs/adr/0002-dual-ingestion-transcripts-and-otel.md
