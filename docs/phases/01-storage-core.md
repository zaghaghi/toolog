# Phase 1 — Storage core

**Goal:** the schema exists, migrates cleanly, and round-trips data. Everything is testable
headlessly; no ingestion or UI yet.

**Depends on:** Phase 0. **Unblocks:** Phases 2, 3.
**Governed by:** [ADR-0003](../adr/0003-sqlite-as-the-embedded-store.md),
[ADR-0004](../adr/0004-store-raw-project-normalized.md).

## Tasks

- [ ] **1.1** `toolog-core`: open SQLite via `rusqlite` with the `bundled` feature. Set
  `journal_mode=WAL`, `foreign_keys=ON`, `synchronous=NORMAL`, and a busy timeout.
  Database path `~/Library/Application Support/toolog/toolog.db`, resolved via `directories`
  so Linux and Windows paths are correct from day one (Phase 8 depends on this).
- [ ] **1.2** Versioned migrations, embedded in the binary — `refinery`, or a hand-rolled
  `user_version` stepper. Migrating from empty and migrating an existing database must both be
  covered by tests from the start.
- [ ] **1.3** `raw_event` table and its append-only insert path:
  `lane`, `source_ref`, `source_offset`, `content_sha256 UNIQUE`, `ingested_at`, `body`.
  Re-inserting identical content must be a silent no-op — this is what makes the Phase 2 tailer's
  rescan-from-zero recovery safe.
- [ ] **1.4** Projection tables: `session`, `tool_call`, `file_change`, `api_request`, `prompt`,
  `permission_mode_change`. Full column list in the plan's data-model section.
- [ ] **1.5** `tool_call_fts` (FTS5) over `tool_name`, `input_summary`, `target_path` and extracted
  result text, kept in sync by triggers. Use `contentless` or an external-content table — measure
  both against the real corpus size before choosing.
- [ ] **1.6** Order-independent upsert helpers per ADR-0009: each lane writes only its own columns
  and sets its `provenance` bit; neither clobbers the other's.
- [ ] **1.7** Typed query layer — the only place SQL is written:
  `timeline_page`, `tool_call_detail`, `list_sessions`, `search`, `stats_*`, `reconcile`.
  Takes typed filter structs, returns typed rows.
- [ ] **1.8** Re-projection entry point: rebuild all projection tables from `raw_event`. This is the
  escape hatch ADR-0004 exists for and must work from day one, not be retrofitted.
- [ ] **1.9** Storage measurement harness: ingest the local 42.2 MB corpus and record database size,
  index size and query latency. **This number decides whether Phase 7 needs to store oversized
  result bodies by reference** — measure before designing that.

## Tests

- [ ] Migrate from empty; migrate twice (idempotent); migrate an already-populated database.
- [ ] `raw_event` dedup: inserting the same body twice yields one row.
- [ ] FTS returns expected matches with ranking; special characters in shell commands do not break
  the query (`|`, `*`, `"`, `-` are common in commands and are FTS5 operators).
- [ ] Concurrent read during write under WAL does not block or return partial rows.
- [ ] Re-projection from `raw_event` reproduces byte-identical projection tables.

## Exit criteria

- A fixture set of raw events can be inserted, projected, queried, wiped and re-projected to the
  same result.
- Storage cost per 1,000 tool calls is a known number, recorded in this file.
