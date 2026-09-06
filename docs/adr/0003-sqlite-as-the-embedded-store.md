# ADR-0003 — SQLite (rusqlite, bundled) as the embedded store

- **Status:** Accepted
- **Date:** 2026-09-04
- **Relates to:** [ADR-0001](0001-tauri-2-for-the-desktop-shell.md), [ADR-0004](0004-store-raw-project-normalized.md), [ADR-0007](0007-single-resident-process.md)

## Context

The brief calls for an embedded database sitting between the collector and the GUI. The workload is
mixed and known:

- **Write:** append-heavy, bursty, low volume. A busy Claude Code session produces on the order of
  hundreds of tool calls per hour. Bodies can be large — the local corpus is 42.2 MB of transcripts.
- **Read:** paged timeline scans, exact lookups by `tool_use_id`, full-text search over commands and
  file paths, and grouped aggregation for the analytics view (cost by project/day, p50/p95 duration,
  error rate by tool).

Full-text search and aggregation are requirements of two of the four views, not optional extras.

> **Phase 9.** The analytics view is gone ([ADR-0010](0010-no-cost-reporting.md)); the aggregation
> requirement is not. It moved rather than lapsed — the risk review groups findings by project, and
> the timeline's activity histogram ([Phase 10](../phases/10-one-lens.md)) buckets the same filtered
> selection the list reads. Full-text search over commands and file paths is untouched.

The single-writer property from ADR-0007 removes the usual multi-process contention concern: one
process owns the only write handle.

## Decision

**SQLite via `rusqlite` with the `bundled` feature.** Opened in WAL mode, with `foreign_keys=ON` and
a busy timeout. FTS5 for search.

**The Rust core owns the only database handle.** The frontend has no SQL access and reaches data
exclusively through typed Tauri commands. `tauri-plugin-sql` is deliberately not used.

All SQL lives in `toolog-core` behind a typed query layer; no other crate writes SQL.

## Consequences

**Positive**

- `bundled` compiles SQLite from source, so there is no system dependency and no version skew across
  user machines — which matters directly for the single-artifact install.
- FTS5 gives search across commands, paths and result text without a second index to maintain.
- Window functions and `GROUP BY` cover the analytics view without hand-rolled aggregation.
- WAL lets the UI read while ingestion writes, with no reader-blocking.
- One file to back up, inspect with any `sqlite3` binary, or delete on uninstall. Inspectable
  evidence is a virtue for an audit tool.

**Negative**

- Compiling SQLite lengthens clean builds.
- Large result bodies stored inline will grow the file; Phase 1 measures this against the real
  corpus, and Phase 7 adds retention caps and storing oversized results by reference.
- Keeping SQL confined to one crate is a discipline the codebase must hold to, not something the
  compiler enforces.

**Rationale for the frontend restriction:** exposing SQL to the WebView would put query construction
on the least-trusted side of the process and make the schema a de-facto public API, blocking the
re-projection that ADR-0004 depends on. Typed commands keep the schema free to change.

## Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| `redb` (pure Rust) | Avoids the C build, but has no SQL, no full-text search and no aggregation. Every timeline filter, search and chart query becomes hand-written traversal. |
| DuckDB | Excellent for the analytics view, oversized and awkward for row-level lookups and point writes, and a much heavier dependency for a resident desktop app. |
| `tauri-plugin-sql` | Would put the database in the frontend, conflicting with the single-writer model and freezing the schema as a UI contract. |
| Plain JSONL files + in-memory index | Rebuilding the index on every launch over a growing corpus, and no durable query surface. |
