# The database

One SQLite file. No server, no schema registry, no ORM — a file you can open with `sqlite3` and
check every claim this tool makes about itself.

| | |
|---|---|
| **Path** | `~/Library/Application Support/toolog/toolog.db` (macOS; resolved through `directories`, so Linux is `~/.local/share/toolog/`) |
| **Engine** | SQLite, bundled with the binary via `rusqlite` — no system SQLite is used |
| **Schema version** | `PRAGMA user_version`, currently **6** |
| **Journal** | WAL, `synchronous = NORMAL`, `foreign_keys = ON`, 5-second busy timeout |
| **Decision** | [ADR-0003](adr/0003-sqlite-as-the-embedded-store.md) |

WAL is what lets the window read while ingestion writes. `NORMAL` is its standard pairing: durable
across a process crash, at risk only from an OS-level crash mid-write. All SQL in the workspace lives
in `toolog-core`; other crates reach data through typed functions and never write a query of their
own.

## The organising principle

**`raw_event` is the evidence. Everything else is a re-runnable projection of it** —
[ADR-0004](adr/0004-store-raw-project-normalized.md).

Neither input format is a stable contract. One ordinary user's history already spans 12 Claude Code
versions, 21 record types, and three different shapes of `toolUseResult`. A parser written against
today's shapes *will* meet tomorrow's, and for an audit tool the question is only what happens to the
data when it does. So every input record is written verbatim before anything parses it, and the
parsed tables can be dropped and rebuilt at any time (`project::reproject`).

That splits the schema into three kinds of table, and knowing which kind you are looking at answers
most questions about it:

| Kind | Tables | Can it be rebuilt? |
|---|---|---|
| **Evidence** | `raw_event` | No — it *is* the source |
| **Projection** | `session`, `tool_call`, `file_change`, `api_request`, `prompt`, `permission_mode_change`, `tool_call_fts` | Yes, from `raw_event` |
| **Record of what happened** | `rule_dismissal`, `deletion`, `rule_sighting` | **No** — see [below](#the-third-kind-things-recomputation-cannot-recover) |

## Evidence

### `raw_event`

Append-only. Written once, never updated, deleted only by explicit retention.

| Column | Notes |
|---|---|
| `id` | Insertion order, and the order the integrity chain is walked in |
| `lane` | `transcript` or `otlp`, checked by the schema |
| `source_ref` | The transcript file path, or the OTLP batch |
| `source_offset` | Byte offset in that file, for tracing a row back to its line |
| `content_sha256` | **UNIQUE.** Makes re-ingestion idempotent, which is what lets the tailer recover from a truncated file by rescanning it from zero |
| `ingested_at` | |
| `body` | The record, verbatim |
| `chain_sha256` | The integrity chain (migration 004). Nullable: rows written before that migration are sealed by a Rust pass, not by SQL |

## The projection

### `session`

One row per Claude Code session: `project_path`, `cwd`, `git_branch`, `cc_version`, `transcript_path`
and the window it was active in.

`agent_name` and `slug` need care, because Phase 2 found three different things being conflated
(migration 002): `agentId` is a subagent *instance*, `attributionAgent` is a subagent *type*
("Explore"), and `agent-name` is a *session label* unrelated to subagents. This column is the third.

### `tool_call`

The centre of the schema — one row per invocation, **assembled from both lanes**.

Its columns divide by which lane owns them, and that division is
[ADR-0009](adr/0009-correlate-on-tool-use-id.md):

| Owner | Columns |
|---|---|
| Transcript | `input_json`, `input_summary`, `target_path`, `result_json`, `result_text`, `result_size`, `success`, `is_sidechain`, `permission_mode` |
| OTLP | `duration_ms`, `error_type`, `decision`, `decision_source` |
| Either | `session_id`, `prompt_id`, `tool_name`, `called_at`, … |

Neither lane overwrites the other's columns. OTEL truncates tool inputs at 512 characters, so letting
it write `input_json` would destroy exactly the evidence the second lane exists to preserve. Upserts
are order-independent and tests assert both arrival orders converge on the same row.

**`provenance` is a two-bit mask** — bit 1 transcript, bit 2 OTLP — recording which lanes witnessed
the call. A row with only bit 1 is a **gap in collection**: the app was not running or the OTLP
configuration is broken. A row with only bit 2 had no transcript body written.

> A refusal is read from the `decision` column and **never** from provenance. The original ADR
> assumed denied calls left no transcript record; Phase 4 measured a real denial and found
> `provenance = 3`. The correction is recorded in ADR-0009 rather than quietly fixed.

**`session_id` deliberately carries no foreign key.** The lanes race: an OTLP decision can arrive
before the transcript line that creates the session, and a hard constraint would reject the very
rejected-call rows this tool exists to capture. `file_change` *does* carry one, with
`ON DELETE CASCADE`, because its ordering is controllable.

Large results are kept by reference rather than copied. Past `RESULT_BODY_LIMIT` (64 KiB),
`result_json` holds a `{"$evidence": …}` marker instead of the body — the evidence store already has
it. Phase 7 measured why: 48 of 3,506 results (1.4%) held 11 MB of 18 MB.

### `file_change`

One row per file an `Edit`/`Write` touched, from `structuredPatch`: `lines_added`, `lines_removed`,
and the patch itself. Cascades from `tool_call`.

### `api_request`

OTLP only — model, tokens, cost, duration. **Captured and never displayed.** That is a scope decision
recorded in [ADR-0010](adr/0010-no-cost-reporting.md), not an omission; `toolog verify` counts the
rows so a lane that stops arriving is still noticed.

### `prompt`

Length and command name. **There is deliberately no column for prompt text.**
[ADR-0008](adr/0008-local-only-zero-egress.md) never sets `OTEL_LOG_USER_PROMPTS`, and the absence of
a column is a stronger guarantee than the absence of a write.

### `permission_mode_change`

Mode transitions within a session, which the risk rules read. Phase 6 measured that OTEL carries no
`permission_mode` attribute at all across 987 records and 11 event types — the mode is a transcript
fact, and the column moved lanes because of it.

### `tool_call_fts`

FTS5 over `tool_name`, `input_summary`, `target_path` and `result_text`, with three triggers keeping
it in step.

**External-content**, not contentless: the text is not stored twice, *and* `snippet()`/`highlight()`
still work, which contentless mode cannot offer and the timeline's match highlighting needs.
Tokenizer is `unicode61 remove_diacritics 2`.

Search text is sanitized before it reaches FTS5 (`fts::build_query`). The corpus is two-thirds shell
commands, and `rm -rf` is FTS5 syntax before it is a search term.

## The third kind: things recomputation cannot recover

These three tables are the exception to "everything derived is rebuildable", and they are the
exception for the same reason. A derivation can be recomputed; an **observation or a judgement**
cannot. Re-running the rules does not recover what someone decided, what was deleted, or when a thing
was first noticed.

### `rule_dismissal`

`(rule_id, note, dismissed_at)`. A judgement a person made about a rule, with the reason they gave.
Re-running the rules must not discard it. It never touches, hides or deletes the calls behind it —
those stay in the timeline and in every export.

### `deletion`

What a purge removed, why, when, and the chain values either side of the hole. Deleting evidence
breaks the integrity chain and *should*; what it must not do is leave the break unexplained.
`toolog verify --chain` reads this to report a break as accounted-for or not, which is the difference
between routine retention and tampering. **Never purged** — a retention policy that eventually erased
the record of its own deletions would be self-defeating.

### `rule_sighting`

`(rule_id, tool_use_id, first_seen)`, `WITHOUT ROWID`, primary key on the pair.

Findings themselves are **not** stored — see [ADR-0011](adr/0011-memoize-the-risk-review.md) and
[ADR-0012](adr/0012-store-sightings-not-findings.md). A `finding` table goes stale on every *ingest*,
not merely on a rule change, and a stored row saying "rule X matched call Y" becomes a claim about a
rule that no longer exists the moment X is retuned. Re-analysing everything costs 95 ms, so there is
no performance argument for storing them either.

What a sighting records is *when a review first noticed*, which nothing can recompute. A row says
"this was flagged, then" — never "this is flagged" — so it cannot go stale: retune a rule and the old
sightings remain true statements about what the old rule saw.

Two properties worth knowing:

- **No foreign key, never purged.** The row must outlive the call it names, exactly as `deletion`
  outlives what it describes. "This was flagged before you deleted it" is a thing an audit trail
  should still be able to say.
- **Written by a review, not by ingestion.** `first_seen` means *first seen by a review*. Recording
  on ingest would mean running twelve rules on the live path, which is the cost Phase 11 exists to
  have removed.

## Integrity

Each `raw_event` carries `chain_sha256` — a hash of a digest of itself linked to the hash of the
record before it. Editing any row (its body, its source, its ingestion time, its position) breaks
every chain value after it. `toolog verify --chain` walks it.

SQLite has no SHA-256 of its own, so sealing is a Rust pass in `id` order by the process holding the
write connection. The partial index `raw_event_unchained` answers "what is not yet sealed" without a
scan.

Integrity is a **separate and lesser claim than completeness**. A record that was never captured
leaves nothing to tamper with, which is why `verify` reports collection gaps as well as chain breaks.

## Retention

`toolog purge` deletes nothing without `--apply`, and prints the sessions it would remove first.

Per session it removes `tool_call` (cascading to `file_change`), `api_request`, `prompt`,
`permission_mode_change`, the session's `raw_event` rows by `source_ref`, and the `session` row. An
age- or size-scoped purge also drops OTLP `raw_event` rows older than the cutoff.

**What survives:** `rule_dismissal`, `deletion`, `rule_sighting`. The third kind of table outlives
what it describes, by design.

## Concurrency

One process owns the only write handle ([ADR-0007](adr/0007-single-resident-process.md)). Alongside
it the window holds **two** read connections:

| Connection | Used by |
|---|---|
| Writer | Ingestion, dismissals, sightings — everything that mutates |
| Reader | The timeline, the histogram, the detail pane, export |
| Risk reader | The risk review only |

The third exists for two reasons, and the second is not a convenience: a slow review must not hold
the timeline behind the shared mutex, **and** `PRAGMA data_version` — which guards the memoized risk
review — reports commits by *other* connections. Read on the writing connection it would never move,
and the memo would never expire. That is asserted in
`toolog-core/tests/roundtrip.rs::data_version_moves_for_another_connections_writes_and_not_for_its_own`,
not assumed.

## Migrations

Embedded with `include_str!`, applied in order, each inside a transaction that bumps `user_version`
in the same transaction — so a failure leaves the database at the previous version rather than
half-migrated.

| # | What it added |
|---|---|
| 001 | The initial schema: evidence, projection, FTS |
| 002 | `tool_call.agent_id`, `session.slug` — subagent attribution |
| 003 | `rule_dismissal` |
| 004 | `raw_event.chain_sha256` — the integrity chain |
| 005 | `deletion` — the record of what was removed |
| 006 | `rule_sighting` — when a finding was first seen |

## What it looks like in practice

The owner's store, measured at one moment — it is a live store and grows while you look at it:

```
~4,500 tool calls · 43 sessions · ~26,500 raw events · ~160 MB
```

| Share | Object |
|---|---|
| 67.4% | `raw_event` (105.5 MiB) |
| 24.2% | `tool_call` (37.9 MiB) |
| 2.8% | `tool_call_fts_data` |
| 2.0% | `raw_event_source` index |
| 3.6% | everything else |

Two thirds of the file is the evidence store, which is the shape ADR-0004 predicts and accepts: the
projection is a rounding error next to keeping every input record verbatim.

## Reading it yourself

The file is plain SQLite on purpose — that is what lets you check the claims in
[PRIVACY.md](../PRIVACY.md) without trusting this program. Encryption at rest was evaluated in
Phase 7 and declined in favour of FileVault, for exactly this reason.

```sh
sqlite3 ~/Library/Application\ Support/toolog/toolog.db

-- What was refused, and who refused it
SELECT called_at, tool_name, decision_source, input_summary
FROM tool_call WHERE decision = 'reject' ORDER BY called_at DESC;

-- Calls only one lane witnessed (1 = transcript only, 2 = OTEL only)
SELECT provenance, count(*) FROM tool_call GROUP BY provenance;

-- What a rule has flagged, and when it was first seen
SELECT rule_id, count(*), datetime(min(first_seen)/1000, 'unixepoch')
FROM rule_sighting GROUP BY rule_id;

-- Cost, which is captured and never displayed (ADR-0010)
SELECT model, sum(cost_usd_micros)/1e6 AS usd FROM api_request GROUP BY model;
```

Read-only inspection is safe while toolog is running — that is what WAL is for. Writing to it is not:
one process owns the write handle.
