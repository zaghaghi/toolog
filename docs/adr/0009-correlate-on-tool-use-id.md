# ADR-0009 — Correlate on `tool_use_id`; treat lane disagreement as a finding

- **Status:** Accepted
- **Date:** 2026-09-04
- **Relates to:** [ADR-0002](0002-dual-ingestion-transcripts-and-otel.md), [ADR-0004](0004-store-raw-project-normalized.md)

## Context

ADR-0002 established two ingestion lanes carrying complementary halves of each tool call. They must
be joined into single rows, and the join has to be exact — an audit trail assembled by heuristic
matching is not an audit trail.

Inspection of both formats shows the identifiers already line up:

| Identifier | Transcript | OTEL |
|---|---|---|
| Tool call | `tool_use.id` and `tool_result.tool_use_id` (`toolu_…`) | `tool_use_id` on `tool_result` and `tool_decision` |
| Prompt turn | `promptId` | `prompt.id` |
| Session | `sessionId` | `session.id` |
| Message | `uuid` | `message.uuid` |

`tool_use_id` is unique per invocation and present in every lane. No fuzzy matching on timestamps,
tool names or argument similarity is needed anywhere.

The lanes are asynchronous and unordered relative to each other. OTEL batches on an interval
(2000 ms per ADR-0006) while transcripts are tailed via fsevents, so either can arrive first.

The more interesting question is what to do when a call appears in only one lane. That is not noise
— each direction carries specific meaning:

- **OTEL only** — no transcript body was written for the call.
- **Transcript only** — a **collection gap**. The app was not running, was paused, or the OTLP
  configuration is broken.

> **Correction, Phase 4 — measured, not assumed.** This ADR originally said "OTEL only ⇒ the call
> was rejected; denied calls leave no transcript record whatsoever." **That is false.** A live
> denial in Phase 4 (`--permission-mode dontAsk`, a `Bash` and a `Read` call both refused) produced
> `provenance = 3`: the transcript keeps the `tool_use` block *and* a `tool_result` whose content is
> the refusal message.
>
> So a refusal is identified by the **`decision` column**, which only the OTLP lane supplies — never
> by the absence of a transcript. The OTLP lane still earns its place, for a sharper reason than the
> original claim: the transcript records only that something was refused, in English, inside a
> result string. It carries no `decision`, no `decision_source`, and no way to ask "which calls did
> a config rule auto-deny?" without parsing prose.
>
> One case remains untested: an **interactive** refusal, where a person presses no at the prompt.
> It is plausible that aborts before any `tool_result` is written, which would make that case
> genuinely OTEL-only. Driving the interactive TUI was out of reach here; until it is measured,
> nothing in the code infers a rejection from provenance.

## Decision

**Join on `tool_use_id`. Make each lane's contribution explicit, and report divergence rather than
smoothing it over.**

1. `tool_call.tool_use_id` is the primary key. Both lanes upsert into it.
2. Each lane writes only its own columns. The transcript lane owns `input_json`, `result_json`,
   `input_summary`, `target_path`, `is_sidechain`. The OTLP lane owns `duration_ms`, `error_type`,
   `decision`, `decision_source`.
3. A `provenance` bitmask records which lanes witnessed the call — bit 1 transcript, bit 2 OTLP.
4. **Upserts are order-independent.** Either lane may create the row; neither overwrites the other's
   columns. Tests assert both arrival orders converge on the same row.
5. **`toolog verify` reports divergence as findings**, with a per-session completeness figure:
   transcript-only calls listed as gaps, OTEL-only calls listed as bodies never written, and
   **refusals counted separately from `decision`** rather than inferred from provenance.
6. Secondary correlation: `prompt_id` groups a turn, `session_id` groups a session, `parent_uuid`
   and `isSidechain` (with `agent-name` records) attribute subagent work — **271 of 2,334 calls in
   the local corpus are sidechain**, so this is not an edge case.

## Consequences

**Positive**

- The join is exact. No timestamp windows, no fuzzy matching, no silent mis-attribution.
- Rejected calls are captured as first-class rows carrying *who* denied them and *why* — structured
  facts the transcript records only as prose, if at all.
- `provenance` makes every row's evidentiary basis explicit, which is what lets the tool state its
  own completeness instead of assuming it.
- Reconciliation turns the redundancy of two lanes into a self-check, so capture failures surface
  loudly rather than as quietly missing rows.

**Negative**

- Rows are transiently incomplete between the two lanes arriving. The UI must render a half-populated
  row gracefully (missing duration, pending decision) rather than hiding it.
- A rejected call and a genuine collection gap both begin as one-lane rows; only the direction
  distinguishes them, so the semantics must be applied carefully and re-evaluated after retention
  pruning.

**Neutral**

- If Claude Code ever changes `tool_use_id` semantics, ADR-0004's raw-first storage allows
  re-projection under a new correlation rule without data loss.

## Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| Match on (session, tool name, timestamp window) | Fragile and wrong under parallel tool calls, which are routine. Exact ids exist; there is no reason to guess. |
| Keep lanes in separate tables and join at query time | Pushes the join into every query and every view, and makes the completeness check harder rather than easier. |
| Let the last writer win on all columns | Truncated OTEL inputs would clobber full transcript inputs — destroying exactly the evidence ADR-0002 exists to preserve. |
| Silently reconcile divergence | Discards the strongest signal the dual-lane design produces: that capture is or is not complete. |
