# Phase 3 — OTLP collector

**Goal:** live Claude Code sessions land permission decisions, durations and cost. This is the
decision lane of ADR-0002 — the half transcripts cannot provide.

**Depends on:** Phase 1. **Unblocks:** Phase 4 (`doctor` verifies this endpoint), Phase 6.
**Governed by:** [ADR-0005](../adr/0005-embedded-otlp-receiver.md),
[ADR-0009](../adr/0009-correlate-on-tool-use-id.md).

## Tasks

- [x] **3.1** `axum` server bound to **`127.0.0.1:47318`** — loopback explicitly, never `0.0.0.0`
  (ADR-0008). Routes:
  - `POST /v1/logs` — the signal that matters
  - `POST /v1/metrics` — accept and drop, returning 204 so a user who enables metrics does not see
    connection errors in their Claude Code debug log
  - `GET /healthz` — used by `doctor` and the tray indicator
- [x] **3.2** Decode **`http/protobuf`** via `opentelemetry-proto` (prost) — the installed default —
  **and `http/json`**, branching on `Content-Type`. Reject unknown content types with a clear 415.
- [x] **3.3** Persist every `LogRecord` to `raw_event` **before** normalizing (ADR-0004), so an
  unrecognized future event type is stored rather than dropped.
- [x] **3.4** Attribute mapping to typed events:
  - `claude_code.tool_result` → `tool_use_id`, `success`, `duration_ms`, `error_type`,
    `tool_input_size_bytes`, `tool_result_size_bytes`, `decision_source`, `mcp_server_scope`
  - `claude_code.tool_decision` → `decision`, `tool_source`, `source`
    (`config` / `hook` / `user_permanent` / `user_temporary` / `user_abort` / `user_reject`)
  - `claude_code.api_request`, `api_error`, `api_refusal` → `api_request` rows: model,
    `cost_usd_micros`, token counts, `duration_ms`, `speed`, `effort`, `query_source`, `agent.name`
  - `claude_code.user_prompt` → `prompt` rows (**length and command name only — never prompt text**,
    which is not enabled per ADR-0006)
  - `claude_code.permission_mode_changed` → `permission_mode_change` rows
  - `claude_code.mcp_server_connection`, `claude_code.session.count`
- [x] **3.5** Common attribute extraction: `session.id`, `prompt.id`, `message.uuid`,
  `event.timestamp`, `event.sequence`, `app.version`, `workspace.host_paths`.
- [x] **3.6** Upsert into `tool_call` by `tool_use_id`, setting the OTLP provenance bit and writing
  **only OTLP-owned columns** — never touching `input_json` or `result_json` (ADR-0009; truncated
  OTEL input must never overwrite full transcript input).
- [x] **3.7** **Create a decision-only row when no transcript row exists.** This is how rejected
  calls enter the database, and the single most valuable thing this lane contributes.
- [x] **3.8** Port conflict handling: probe at startup and fall back to the next free port
  (`port::choose`). ⚠️ **Half of this task is Phase 4's**: writing the chosen port into the
  `settings.json` `env` block needs `doctor`, which does not exist yet. `CollectorHandle::endpoint()`
  exposes the value for it to consume.
- [x] **3.9** Backpressure and limits: request body size cap, bounded ingest queue, and a metric for
  dropped records surfaced in the tray rather than swallowed.

## Tests

- [x] Golden OTLP payloads in **both encodings** → expected rows.
- [x] **Out-of-order convergence:** OTEL-before-transcript and transcript-before-OTEL produce an
  identical final row. This is the core correctness property of ADR-0009.
- [x] A `tool_decision` with `decision=reject` and no transcript counterpart yields a row with
  `provenance = otlp only`.
- [x] Malformed protobuf, truncated body and wrong content type all return errors without panicking
  or killing the listener.
- [x] Loopback bind asserted — the socket is not reachable from a non-loopback address.

## Exit criteria

- [x] A real Claude Code session populates `duration_ms` and `decision_source`. **Verified
  live** — see below. (`doctor --fix` is Phase 4, so the variables were exported for one
  command instead.)
- [ ] ⚠️ **Denying a tool call produces `decision=reject` with no transcript body.** Covered by
  tests, **not verified live**: the account hit its session limit before a refusal could be
  provoked. Do this first in Phase 4, once `doctor` makes the setup one command.
- [x] Stopping and restarting the receiver mid-session loses no already-received events.
  (`tests/server.rs::restarting_the_receiver_keeps_what_it_had`)

## Outcome

**118 tests** workspace-wide, `clippy -D warnings` and `fmt --check` clean.

One crate serves both wire formats: `opentelemetry-proto` with `gen-tonic-messages` for prost
and `with-serde` for OTLP/JSON, verified against the JS exporter's int64-as-string encoding
before being adopted. Roughly 12 new transitive crates, measured rather than assumed.

### Verified against a live session

Exporting the variables for one `claude -p` run produced real rows:

| | |
|---|---|
| Tool calls | 3, with `decision=accept`, `decision_source=config` |
| Durations | 5 ms, 32 ms, 15 ms — available from no other source |
| API requests | 6, one costing **$0.169**, with model and token counts |
| Provenance | `2` (OTLP only), correctly, as no transcript was read |

### What live traffic corrected

**The documented endpoint was wrong, and it captured nothing.** [ADR-0006] specified
`OTEL_EXPORTER_OTLP_LOGS_ENDPOINT=http://127.0.0.1:47318`. The OTLP spec says a per-signal
endpoint "MUST be used as-is without any modification" — unlike the generic variable, the SDK
appends nothing. The exporter posted to `/`, got a 404, and said nothing above debug level.
**This would have shipped an install that silently captured nothing.** The ADR is corrected,
and the receiver now also accepts logs on `/` so a base-URL-shaped value still works.

**The event name is not what the reference implies.** The docs name events
`claude_code.tool_result`; the `event.name` *attribute* carries the bare `tool_result`, while
the record **body** carries the qualified form. Matching the documented name projected zero
rows from a batch of 11 records that had arrived correctly. Names are now normalized by
stripping the prefix, and the test doubles were rewritten to match the live wire format.

**Undocumented event types arrive**: `plugin_loaded`, `hook_registered`, `assistant_response`.
All are stored as evidence and counted as unprojected, which is the behaviour [ADR-0004] exists
to give.

**`tool_decision` carries no `tool_input`** — only `tool_result` does. Since a rejected call
emits a decision and never a result, its `tool_parameters` attribute is the only place its
target can come from. Both are now read.

### A capability that would otherwise have been lost

A rejection has no transcript, so without OTEL's attempted input the audit trail would record
that *a Bash call* was refused while saying nothing about what it would have done. `OtelFacts`
now carries the attempted input, merged **existing-wins** — the reverse of every other field —
so it fills a hole where no transcript exists and can never displace one that does. Both
directions are tested.

This moved `normalize` from `toolog-ingest` into `toolog-core`, where [ADR-0003] already named
normalization as core's job: both lanes see the same tool names and inputs.

### Privacy note for Phase 7

Every event carries `user.email`, `user.id`, `user.account_uuid` and `organization.id`. It
stays local by construction ([ADR-0008]) and is the user's own identity on their own machine,
but `PRIVACY.md` should say so plainly. Confirmed working as intended: `prompt` arrived as
`<REDACTED>` because `OTEL_LOG_USER_PROMPTS` is deliberately never set.

### Fixtures

`fixtures/otlp/live-session-events.jsonl` holds one exemplar of each event type from a real
session, with identity and paths scrubbed and structure exact. Unlike transcripts — 37 MB of
free text that cannot be reliably redacted — an OTLP event's attributes are few and fully
enumerable, so these commit safely. Distinct identifiers are scrubbed to distinct values so
rows that were separate stay separate.

[ADR-0003]: ../adr/0003-sqlite-as-the-embedded-store.md
[ADR-0004]: ../adr/0004-store-raw-project-normalized.md
[ADR-0006]: ../adr/0006-configure-via-settings-env-block.md
[ADR-0008]: ../adr/0008-local-only-zero-egress.md
