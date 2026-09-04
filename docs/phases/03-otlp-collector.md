# Phase 3 — OTLP collector

**Goal:** live Claude Code sessions land permission decisions, durations and cost. This is the
decision lane of ADR-0002 — the half transcripts cannot provide.

**Depends on:** Phase 1. **Unblocks:** Phase 4 (`doctor` verifies this endpoint), Phase 6.
**Governed by:** [ADR-0005](../adr/0005-embedded-otlp-receiver.md),
[ADR-0009](../adr/0009-correlate-on-tool-use-id.md).

## Tasks

- [ ] **3.1** `axum` server bound to **`127.0.0.1:47318`** — loopback explicitly, never `0.0.0.0`
  (ADR-0008). Routes:
  - `POST /v1/logs` — the signal that matters
  - `POST /v1/metrics` — accept and drop, returning 204 so a user who enables metrics does not see
    connection errors in their Claude Code debug log
  - `GET /healthz` — used by `doctor` and the tray indicator
- [ ] **3.2** Decode **`http/protobuf`** via `opentelemetry-proto` (prost) — the installed default —
  **and `http/json`**, branching on `Content-Type`. Reject unknown content types with a clear 415.
- [ ] **3.3** Persist every `LogRecord` to `raw_event` **before** normalizing (ADR-0004), so an
  unrecognized future event type is stored rather than dropped.
- [ ] **3.4** Attribute mapping to typed events:
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
- [ ] **3.5** Common attribute extraction: `session.id`, `prompt.id`, `message.uuid`,
  `event.timestamp`, `event.sequence`, `app.version`, `workspace.host_paths`.
- [ ] **3.6** Upsert into `tool_call` by `tool_use_id`, setting the OTLP provenance bit and writing
  **only OTLP-owned columns** — never touching `input_json` or `result_json` (ADR-0009; truncated
  OTEL input must never overwrite full transcript input).
- [ ] **3.7** **Create a decision-only row when no transcript row exists.** This is how rejected
  calls enter the database, and the single most valuable thing this lane contributes.
- [ ] **3.8** Port conflict handling: probe at startup; on collision select an alternative and
  rewrite **both** the app config and the `settings.json` `env` block together (ADR-0006 — if they
  disagree, capture silently stops).
- [ ] **3.9** Backpressure and limits: request body size cap, bounded ingest queue, and a metric for
  dropped records surfaced in the tray rather than swallowed.

## Tests

- [ ] Golden OTLP payloads in **both encodings** → expected rows.
- [ ] **Out-of-order convergence:** OTEL-before-transcript and transcript-before-OTEL produce an
  identical final row. This is the core correctness property of ADR-0009.
- [ ] A `tool_decision` with `decision=reject` and no transcript counterpart yields a row with
  `provenance = otlp only`.
- [ ] Malformed protobuf, truncated body and wrong content type all return errors without panicking
  or killing the listener.
- [ ] Loopback bind asserted — the socket is not reachable from a non-loopback address.

## Exit criteria

- With `doctor --fix` applied, running a real Claude Code session populates `duration_ms` and
  `decision_source` on new rows.
- **Denying a tool call produces a row with `decision=reject` and no transcript body** — the
  proof that this lane earns its place.
- Stopping and restarting the receiver mid-session loses no already-received events.
