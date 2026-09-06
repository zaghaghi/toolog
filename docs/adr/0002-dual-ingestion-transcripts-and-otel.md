# ADR-0002 — Dual ingestion: transcripts for content, OTEL for decisions

- **Status:** Accepted
- **Date:** 2026-09-04
- **Deciders:** Project owner
- **Relates to:** [ADR-0004](0004-store-raw-project-normalized.md), [ADR-0005](0005-embedded-otlp-receiver.md), [ADR-0009](0009-correlate-on-tool-use-id.md)

## Context

The brief's suggested architecture is `Claude-Code -[OTEL]-> Collector -> Embedded Database <- GUI`.
Before committing to it, both available data sources were measured against what an *audit* tool
needs. (The brief originally said "audio"; the owner clarified it was a typo for "audit", which
raised fidelity and completeness above latency in the requirements.)

**What OTEL provides.** With `CLAUDE_CODE_ENABLE_TELEMETRY=1` and `OTEL_LOGS_EXPORTER=otlp`,
Claude Code emits `claude_code.tool_result` (`tool_name`, `tool_use_id`, `success`, `duration_ms`,
`error_type`, input/result sizes, `decision_source`), `claude_code.tool_decision` (`decision`,
`tool_source`, `source`), `claude_code.api_request` / `api_error` / `api_refusal` (model,
`cost_usd_micros`, token counts, `speed`, `effort`), plus `user_prompt`,
`permission_mode_changed` and `mcp_server_connection`. Every event carries `session.id`,
`prompt.id`, `message.uuid` and `event.sequence`.

**Where OTEL falls short.** `tool_input` requires `OTEL_LOG_TOOL_DETAILS=1` and is still truncated:
values over 512 characters are cut, with a payload cap around 4 KB. Result *content* needs a further
`OTEL_LOG_TOOL_CONTENT=1`. **A truncated shell command is not evidence.** For a tool whose purpose is
answering "what exactly did the agent run", this is disqualifying on its own.

**What transcripts provide.** `~/.claude/projects/**/*.jsonl` — 42.2 MB and 2,334 tool calls on the
owner's machine. `assistant` records carry `tool_use` blocks with the complete, untruncated `input`;
the paired `user` record carries `toolUseResult`, including `structuredPatch` diffs for edits.
Envelopes add `cwd`, `gitBranch`, `version`, `isSidechain` and `promptId`.

> **Corrected in Phase 6, by measurement.** `permission_mode_changed` is not emitted, and no OTEL
> event carries a `permission_mode` attribute: 987 records across 11 event types on the owner's
> machine, and the only mode-shaped attribute is `safe_mode`, a boolean. **The permission mode is a
> transcript fact** — a dedicated `permission-mode` record and a `permissionMode` field on each
> `user` turn, with real values `default`, `auto`, `plan`, `acceptEdits` and `dontAsk`. The column
> moved lanes accordingly; see [Phase 6](../phases/06-risk-analytics-live.md).
>
> The same pass confirmed the privacy posture rather than assuming it: `prompt` and `response`
> attributes **are** present on OTLP records, and their value is the literal string `<REDACTED>`,
> because [ADR-0008](0008-local-only-zero-egress.md) never sets `OTEL_LOG_USER_PROMPTS` or
> `OTEL_LOG_ASSISTANT_RESPONSES`.

**Where transcripts fall short.** They contain no permission decision, no `duration_ms`, and no cost
or token data. Most importantly, **a tool call that was denied carries no record of who denied it or
why** — that exists only as a `claude_code.tool_decision` with `decision=reject`. An audit tool that cannot
show what was refused is missing half the security story.

> **Amended in Phase 9 — the cost argument is withdrawn.** toolog captures cost and reports none of
> it ([ADR-0010](0010-no-cost-reporting.md)). Cost and tokens were never the reason for this lane
> and are struck from the case for it. What is left is the reason that was always doing the work:
> `decision` and `decision_source` exist in no other lane, and `duration_ms` is measured in no other
> lane. `api_request` is still ingested and still counted by `toolog verify` — a lane that stops
> arriving is a capture failure whether or not anything renders it.

The two sources are complementary in exactly the places each is weak. Neither is sufficient alone.

A third source, **HTTP hooks** (`"type": "http"`, POSTing the full payload to a local URL), was
evaluated. It would give zero-latency untruncated capture, and the known ECONNREFUSED bug
(anthropics/claude-code#30613, closed as not planned) affects only external URLs — loopback works.

## Decision

**Ingest from two lanes:**

1. **Transcript tail** — the content of record. Full `tool_input` and `toolUseResult`,
   `structuredPatch` diffs, cwd, git branch, sidechain flag. Read-only file watching via fsevents.
2. **OTLP receiver** — the decision and cost layer. Permission decision and its source, rejected
   calls, durations, cost, tokens, model, effort, API errors, MCP connections.

Rows carry a `provenance` bitmask recording which lanes witnessed each call.

**Hooks are excluded from v1.** Their content duplicates the transcript, and they place code on
Claude Code's tool-execution path. Revisit only if reconciliation (ADR-0009) shows the transcript
tail dropping calls.

## Consequences

**Positive**

- Evidence is untruncated, and the permission story is complete including refusals.
- Nothing runs on Claude Code's critical path. Both lanes are passive: a read-only file watch and a
  fire-and-forget network export whose failure only writes a debug-level line.
- The lanes check each other. Divergence is a finding, not an inconsistency to paper over — this is
  how the tool demonstrates completeness rather than asserting it (ADR-0009).
- Backfill and live tail share one parser, so history and live capture cannot drift apart.

**Negative**

- Two parsers, two failure modes, and a join to maintain.
- Rows are assembled from asynchronous sources, so a call can be briefly half-populated. The
  normalizer must be order-independent (an OTEL event may arrive before or after its transcript line).
- The OTEL lane is lossy while the app is not running. Transcripts are read from disk after the
  fact, so only the decision/cost layer has a gap, and `toolog verify` surfaces it explicitly.

## Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| OTEL only (as the brief sketched) | Evidence too lossy — 512-char truncation on tool inputs, result content off by default. Single point of failure with no fallback if the `env` block is clobbered. |
| Transcripts only | No permission decisions, no decision source, no duration. A refusal appears only as English prose inside a result string. Loses the entire risk-review surface. (Cost was listed here too until [ADR-0010](0010-no-cost-reporting.md); the row stands without it.) |
| Add hooks as a third lane | Real tamper-evidence value from a third independent witness, but it sits on the tool-execution path and mostly duplicates transcript content. Deferred, not dismissed. |

## Correction — Phase 4

This ADR originally claimed that "a tool call the user denied leaves no transcript trace at all".
A live denial in Phase 4 disproved it: with `--permission-mode dontAsk`, a refused `Bash` call and a
refused `Read` call both appeared in **both** lanes, the transcript holding the `tool_use` block and
a `tool_result` whose content is the refusal message.

The conclusion of this ADR is unchanged, and the reasoning is now sharper. The transcript says
*that* something was refused, in prose. Only OTEL says **who refused it and under which rule**, as
`decision` and `decision_source` — queryable columns rather than a sentence to grep. The dual-lane
design is what makes "which calls did a config rule auto-approve, and which did it deny?" a query
instead of a text search.

See [ADR-0009](0009-correlate-on-tool-use-id.md) for the consequence that matters in code: a
rejection is identified by its `decision`, never by a missing transcript. The interactive-refusal
case — a person pressing no — remains unmeasured.
