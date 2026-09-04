# Phase 1 — Storage core

**Goal:** the schema exists, migrates cleanly, and round-trips data. Everything is testable
headlessly; no ingestion or UI yet.

**Depends on:** Phase 0. **Unblocks:** Phases 2, 3.
**Governed by:** [ADR-0003](../adr/0003-sqlite-as-the-embedded-store.md),
[ADR-0004](../adr/0004-store-raw-project-normalized.md).

## Tasks

- [x] **1.1** `toolog-core`: open SQLite via `rusqlite` with the `bundled` feature. Set
  `journal_mode=WAL`, `foreign_keys=ON`, `synchronous=NORMAL`, and a busy timeout.
  Database path `~/Library/Application Support/toolog/toolog.db`, resolved via `directories`
  so Linux and Windows paths are correct from day one (Phase 8 depends on this).
- [x] **1.2** Versioned migrations, embedded in the binary — `refinery`, or a hand-rolled
  `user_version` stepper. Migrating from empty and migrating an existing database must both be
  covered by tests from the start.
- [x] **1.3** `raw_event` table and its append-only insert path:
  `lane`, `source_ref`, `source_offset`, `content_sha256 UNIQUE`, `ingested_at`, `body`.
  Re-inserting identical content must be a silent no-op — this is what makes the Phase 2 tailer's
  rescan-from-zero recovery safe.
- [x] **1.4** Projection tables: `session`, `tool_call`, `file_change`, `api_request`, `prompt`,
  `permission_mode_change`. Full column list in the plan's data-model section.
- [x] **1.5** `tool_call_fts` (FTS5) over `tool_name`, `input_summary`, `target_path` and extracted
  result text, kept in sync by triggers. Use `contentless` or an external-content table — measure
  both against the real corpus size before choosing.
- [x] **1.6** Order-independent upsert helpers per ADR-0009: each lane writes only its own columns
  and sets its `provenance` bit; neither clobbers the other's.
- [x] **1.7** Typed query layer — the only place SQL is written:
  `timeline_page`, `tool_call_detail`, `list_sessions`, `search`, `stats_*`, `reconcile`.
  Takes typed filter structs, returns typed rows.
- [x] **1.8** Re-projection entry point: rebuild all projection tables from `raw_event`. This is the
  escape hatch ADR-0004 exists for and must work from day one, not be retrofitted.
- [x] **1.9** Storage measurement harness: ingest the local 42.2 MB corpus and record database size,
  index size and query latency. **This number decides whether Phase 7 needs to store oversized
  result bodies by reference** — measure before designing that.

## Tests

- [x] Migrate from empty; migrate twice (idempotent); migrate an already-populated database.
- [x] `raw_event` dedup: inserting the same body twice yields one row.
- [x] FTS returns expected matches with ranking; special characters in shell commands do not break
  the query (`|`, `*`, `"`, `-` are common in commands and are FTS5 operators).
- [x] Concurrent read during write under WAL does not block or return partial rows.
- [x] Re-projection from `raw_event` reproduces byte-identical projection tables.

## Exit criteria

- [x] A fixture set of raw events can be inserted, projected, queried, wiped and re-projected to
  the same result. (`tests/roundtrip.rs::insert_project_query_wipe_reproject_is_stable`)
- [x] Storage cost per 1,000 tool calls is a known number, recorded below.

## Outcome

27 tests passing, `cargo clippy -D warnings` and `cargo fmt --check` clean.

### Measured against the real corpus

`cargo run --release -p toolog-core --example measure_storage`, over 39 transcript
files (41.2 MiB, 30 sessions, **2,415 tool calls**):

| | |
|---|---|
| Database total | **75.1 MiB** — 1.82x the corpus |
| `raw_event` (evidence) | 47.8 MiB, **63.6%** |
| `tool_call` (projection) | 21.9 MiB, 29.1% |
| FTS index | 2.1 MiB, 2.8% |
| All other indexes | ~3 MiB |
| **Per 1,000 tool calls** | **~31 MiB** |

The per-1,000 figure is corpus-shaped, not per-row: `raw_event` holds all 10,259 records
(assistant text, attachments, envelopes), not only the ones that became tool calls. It is
the right number for capacity planning and the wrong one for a single row's cost.

**The 63.6% is the ADR-0004 tax, and it is worth it.** Storing evidence verbatim costs
roughly two-thirds of the database, and buys the ability to re-derive every projection when
Claude Code changes format — which the same corpus shows happening across 12 versions.

Tool distribution closely tracks the planning figures (Bash 1,617 · Edit 292 · Read 253 ·
Write 70), confirming the projection is finding what the corpus actually contains. The count
exceeds the planned 2,334 because the corpus grew while this project was being built.

**Dedup is not theoretical:** 2,309 of 12,568 lines (18%) were already held by content hash.
Whatever produces them — resumed sessions, replayed context — a tailer that rescans from
zero relies on exactly this.

### Query latency

All sub-3 ms at this corpus size, with `timeline_page` at 0.6 ms and `search` under 1 ms.
No pagination or index work is needed yet; revisit at ~10x.

### Task 1.5 settled by measurement

External-content FTS5 **2.2 MiB** vs contentless **2.7 MiB**. External content is both
*smaller* and the only one of the two that supports `snippet()`/`highlight()`, which Phase 5
needs for match highlighting. Decided on numbers rather than reputation, as the task asked.

### Feeds Phase 7

**30 calls (1.2% of rows) carry results over 64 KiB, totalling 7.1 MiB — 9.4% of the
database.** Median result is 943 B, p95 is 13.4 KiB, max is 610 KiB. So a by-reference
threshold at 64 KiB would move nearly a tenth of the storage out of the main table while
touching barely one row in a hundred. That is the concrete input task 7.5 was waiting for.

### Deviations from the plan

**Migrations are a hand-rolled `PRAGMA user_version` stepper, not `refinery`** — the task
allowed either. refinery-core 0.9.2 caps at `rusqlite <= 0.39` and this workspace is on
0.40.2, so adopting it meant a downgrade or two linked copies of SQLite. The stepper is
~60 lines, runs each migration and its version bump in one transaction, and refuses to open
a database written by a newer build.

**`tool_call.result_text` was added** beyond the plan's data model. The plan called for FTS
over "extracted result text"; with an external-content FTS table that has to be a real
column on `tool_call`.

**`tool_call.session_id` carries no foreign key.** The lanes race — an OTLP decision can
arrive before the transcript line that creates the session — and a constraint there would
reject exactly the rejected-call rows this tool exists to capture. `file_change` keeps its
foreign key, since that ordering is controllable.
