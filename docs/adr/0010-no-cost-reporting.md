# ADR-0010 — toolog does not report cost

- **Status:** Accepted
- **Date:** 2026-09-06
- **Relates to:** [ADR-0002](0002-dual-ingestion-transcripts-and-otel.md),
  [ADR-0008](0008-local-only-zero-egress.md)

## Context

Phase 6 built a usage view: spend, tokens, cache hit ratio, cost by project, cost by model, cost per
day, and a coverage line for every one of them because the OTLP lane is the only lane that records
any of it. It was careful work. It answers a question the owner does not have.

After hours of real use, the owner's report was unambiguous:

> "The usage page is useless to me — I don't want to know about costs in this application; there are
> tons of other applications that do the cost-related things."

That is a scope decision, not a bug report, and it is worth recording as one for a specific reason.
**ADR-0002 justified the OTEL lane partly by the cost it carries.** Its "where transcripts fall
short" is "no permission decision, no `duration_ms`, and no cost", and its rejected-alternatives
table dismisses transcripts-only partly because it "loses the entire risk-review and analytics
surface". Delete the usage view without a record and the second lane looks over-justified — and in a
year someone reads ADR-0002, notices that cost is captured and nothing shows it, and reasonably
builds the view again.

The captured data is not in question. `api_request` records arrive, are stored raw
([ADR-0004](0004-store-raw-project-normalized.md)) and are projected into the `api_request` table
exactly as before. `toolog verify` still counts those rows, because a lane that stopped arriving is
a capture failure whether or not anything renders it.

## Decision

**toolog captures cost and never reports it.** The tool answers "what did the agent do, and who let
it" — not "what did it spend".

1. The usage view, the `usage` and `stats` commands, the `toolog usage` subcommand and
   `toolog-core::analytics` are removed (Phase 9). The chart primitives they alone used go with
   them.
2. `api_request` keeps being ingested, projected and counted. Nothing displays what it holds — not
   in the timeline's session headers, not in the status page, not in the CLI.
3. **ADR-0002 stands on `decision` and `decision_source`.** Those two columns exist in no other
   lane, and they are what makes "which calls did a config rule auto-deny?" answerable without
   parsing English prose out of a result string. Cost was a supporting argument for the OTEL lane
   and is now withdrawn; the lane does not need it. ADR-0002 is amended in place to say so rather
   than superseded, because the decision it records — two lanes — has not changed.
4. `Totals` keeps its `cost_usd_micros` and token sums. They are a census of what the store holds,
   asserted by a test, and read by nothing that renders.

## Consequences

**Positive**

- One less surface to keep honest. Every cost number carried a three-state coverage caveat, because
  a store built from imported transcripts has no cost data and never will — and "$0.00 spent" and
  "we were not watching" are different statements. That caveat had to be right in the view, in the
  CLI renderer, in the tooltips and in the table twin.
- The scope is easier to state: a record of what an agent did, not a bill.
- Roughly 4,000 net lines of Rust, TypeScript and CSS leave the codebase — four chart forms and
  372 lines of stylesheet among them — without taking a capability with them.

**Negative**

- Anyone who did want spend-by-project now needs another tool, and the data to answer it is sitting
  in the store unread. `toolog export` does not reach `api_request`, so it is a SQL query away, not
  a flag away.
- Re-adding the view later means re-writing it. The aggregation was not trivial — active time with
  an idle cutoff, percentiles, per-lane coverage — and it is in the history rather than behind a
  feature flag.

**Neutral**

- The OTLP receiver's work does not change. It already ingests every record type it is sent, and
  ADR-0005's argument for embedding it never mentioned cost.

## Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| Keep the usage view, strip the cost half | The owner named one chart worth keeping — *Tool calls per day* — and asked for it **on the timeline, keyed to the timeline's time range**. A usage view holding one chart that belongs somewhere else is a tab that exists to have something in it. That chart moves in [Phase 10](../phases/10-one-lens.md); what is left over is not a view. |
| Keep it behind a preference, off by default | A switch is a decision deferred, not taken, and both branches still have to be built, styled, tested and kept truthful about coverage. The cost of the view was never the pixels. |
| Stop ingesting `api_request` too | The capture claim is "both lanes, whole". Dropping a record type because nothing currently renders it makes `toolog verify` unable to tell a configuration break from a deliberate omission — and re-adding it later would leave a hole in the history that raw storage exists to prevent. |
| Supersede ADR-0002 rather than amend it | Its decision — two lanes, transcripts for content, OTEL for decisions — is unchanged and still correct. Only one of its supporting arguments is withdrawn. Superseding would imply the lane structure was reconsidered, which it was not. |
