# Phase 6 — Risk, analytics & live views

**Goal:** the remaining three of the four surfaces the owner asked for, built on Phase 5's
infrastructure.

**Depends on:** Phases 3, 4, 5. **Unblocks:** Phase 7 (rules inform redaction).

## Risk & permission review

*"What got approved, and how?"* — the view only the OTEL lane makes possible (ADR-0002).

- [x] **6.1** Rule engine over `tool_call`. **Rules as data, not code**, so new rules ship without a
  release and users can add their own.
- [x] **6.2** Starter rule set:
  - Sessions run under `bypassPermissions` or `dontAsk`
  - Writes outside the session `cwd` or outside any known project root
  - **Auto-approved destructive Bash** (`decision_source = config`): `rm -rf`, `dd`, `mkfs`,
    `curl … | sh`, `git push --force`, history rewrites
  - **Rejected-then-retried-and-accepted** sequences — the agent working around a refusal, and the
    pattern that most justifies this tool existing
  - Secrets read: `.env`, `id_rsa`, `.pem`, `credentials`, keychain access
  - Network-reaching Bash commands (`curl`, `wget`, `nc`, `ssh`, `scp`)
  - MCP tool usage grouped by server
  - Tool calls during a session where permission mode changed mid-flight
- [ ] **6.3** Severity ranking, drill-through from finding to the underlying calls, and
  dismiss-with-note (dismissals stored, never deleting the underlying row).
- [ ] **6.4** Per-project risk summary — the "posture" glance.

## Usage analytics

- [ ] **6.5** Aggregates from `api_request` + `tool_call`: cost and tokens by project / session /
  model / day, tool frequency, error rate by tool, p50/p95 duration, cache-hit ratio, sidechain
  share, active time.
- [ ] **6.6** Charts. **Load the `dataviz` skill before writing any chart code** — palette,
  accessibility and dark-mode behaviour are decided there, not ad hoc.
- [ ] **6.7** Date-range comparison (this week vs last) and cost-per-project leaderboard.
- [ ] **6.8** Honest empty states: cost data exists only for sessions captured live by the OTLP lane,
  never for backfilled history. **Say so in the UI** rather than rendering a misleading zero.

## Live monitoring

- [ ] **6.9** Ingest → Tauri event → UI channel for `live_tool_call`. In-process, no IPC (ADR-0007).
- [ ] **6.10** Concurrent sessions as parallel lanes, each with a running cost meter and current tool.
- [ ] **6.11** Idle/active indication per session; auto-scroll with a pause-on-interaction affordance.
- [ ] **6.12** Native notifications on rejected calls and high-severity rule hits, off by default and
  individually toggleable.

## Progress

**Done: the risk engine and its rules (6.1, 6.2), and the correction they rest on.**
`toolog-core::rules` compiles a TOML rule into bound SQL; `toolog risk` runs it from the command
line. 14 tests in `crates/toolog-core/tests/rules.rs`, plus the permission-mode work below.

### `permission_mode` comes from the transcript, not from OTEL

[ADR-0002](../adr/0002-dual-ingestion-transcripts-and-otel.md) lists `permission_mode_changed` among
the events OTEL emits, and [ADR-0009](../adr/0009-correlate-on-tool-use-id.md) gave the OTLP lane the
`permission_mode` column. Task 6.2's first two rules are about the permission mode, so the first
thing this phase did was look at what actually arrives.

**987 OTLP records across 11 event types, and not one `permission_mode` attribute.** No
`permission_mode_changed` event either. The only mode-shaped attribute Claude Code 2.1.260 sends is
`safe_mode`, a boolean, and it was `false` on all 364 records carrying it. The column was null for
every one of the 3,023 calls in the owner's store, which is why the Phase 5 filter for it could never
offer a value.

The transcript carries it, in two shapes: a dedicated `permission-mode` record, and a
`permissionMode` field on each `user` turn. Real values: `auto`, `plan`, `acceptEdits`, `default`,
`dontAsk`.

So the column changed lanes. `TranscriptFacts` owns it, `OtelFacts` no longer has the field at all,
and the transcript projector follows the mode as records go by, stamping each call with the mode in
force and writing a `permission_mode_change` row where it moves. After re-projecting the owner's
store: **3,023 of 3,023 calls carry a mode, and 55 changes are recorded** in a table that had been
empty since Phase 3.

Two details worth keeping:

- **A bare `permission-mode` record has no uuid and no timestamp**, so two of them with the same
  value are byte-identical and the second is dropped by content-hash deduplication (ADR-0004) — an
  `auto` → `plan` → `auto` return loses its last step. Every `user` record is unique, so the next
  prompt turn resynchronises the mode. Both sources are read for that reason.
- **A subagent's transcript is a separate file carrying no `permission-mode` record**, which left
  271 sidechain calls unstamped. `inherit_permission_modes` fills those from the session's own
  recorded changes, preferring the latest at or before the call. It is the only inference in the
  projection, it fills nulls only, and it says so where it is written.

The same pass confirmed the privacy posture by measurement rather than assumption: `prompt` and
`response` attributes **are** present on OTLP records, and their value is the literal string
`<REDACTED>`, because ADR-0008 never sets `OTEL_LOG_USER_PROMPTS` or `OTEL_LOG_ASSISTANT_RESPONSES`.

The re-projection was verified lossless on an exact copy before being run on the live store:
decisions 224 → 224, durations 222 → 222, api\_requests 220 → 220, cost identical to the micro-dollar,
file changes 384 → 384.

### What the rules found, and the two bugs that finding them exposed

Running the starter set against the real corpus is the only way to know whether a rule set is worth
having. The first run reported 2,432 calls for one rule and flagged two calls that had never run a
destructive command. Both were bugs in the engine, not in the data:

- **A heredoc body is not the command.** `cat > notes.md <<'EOF' … rm -rf … EOF` puts its whole body
  in `input_summary`, so a rule looking for `rm -rf` flagged a call that was *writing documentation
  about* `rm -rf`, and a `.env` rule flagged one writing a `.gitignore` that mentions it. Rules can
  now match `first_line` only, and the command-shaped ones do. Auto-approved destructive commands
  went from 2 findings to 0 — both were this.
- **`_` is a `LIKE` wildcard.** The escaping added backslashes without an `ESCAPE` clause, so
  `id_rsa` compiled to a pattern that could only match a string containing a backslash — it could
  never have fired. Credential findings fell from 17 to 5 once both bugs were fixed, and the five
  that remain are real.
- **A session-scoped rule is about the session.** "Permission mode changed mid-session" listed every
  call in every such session: 2,432 rows, true and useless. It now reports the 14 sessions.

## Exit criteria

- The risk view flags a deliberately auto-approved `rm -rf` in a scratch directory, with drill-through
  to the exact call.
- Analytics totals reconcile against `claude_code.api_request` costs for a live session.
- Two concurrent Claude Code sessions appear as separate live lanes with correct attribution.
