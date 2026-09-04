# ADR-0004 — Store raw envelopes verbatim; normalization is a re-runnable projection

- **Status:** Accepted
- **Date:** 2026-09-04
- **Relates to:** [ADR-0002](0002-dual-ingestion-transcripts-and-otel.md), [ADR-0003](0003-sqlite-as-the-embedded-store.md)

## Context

Neither input format is a stable contract. Both are internal to Claude Code and change without
notice. Profiling the owner's `~/.claude/projects` corpus made the scale of the drift concrete:

- **39 transcript files, 42.2 MB, spanning 12 Claude Code versions** (2.1.161 → 2.1.259) — twelve
  versions of drift accumulated in one ordinary user's history.
- **21 distinct record `type` values**, including several undocumented ones (`atis-latch`,
  `artifact-autoreact-ledger`, `frame-link`, `worktree-state`, `relocated`).
- **`toolUseResult` is polymorphic:** a JSON object 2,171 times, a bare string 99 times, and an
  array 62 times. All three shapes occur in normal use.
- Its keys vary per tool: `stdout`/`stderr`/`interrupted` for Bash, `structuredPatch`/`originalFile`/
  `userModified` for Edit, `file` for Read.

A parser written against today's shapes will meet unknown shapes. The question is only what happens
to the data when it does.

For an audit tool this is sharper than for a normal application: **data lost at ingestion time
cannot be recovered.** If a parser silently drops a field it does not recognise, the evidence is
gone, and the tool's central claim — that it holds a complete record — is false.

## Decision

**Persist every input record verbatim to `raw_event` before any parsing. Treat every other table as
a derived, re-runnable projection of it.**

- `raw_event` is append-only: written once, never updated, deleted only by explicit retention.
- Each row carries its lane, source reference, byte offset, ingestion time, and a
  `content_sha256` unique constraint that makes re-ingestion idempotent.
- Normalization reads `raw_event` and writes `tool_call`, `session`, `file_change`, `api_request`
  and the rest. It can be re-run at any time to rebuild them.
- Unknown record types, unknown tools and unknown OTEL events are stored and skipped by the
  projection — **never rejected, never an error**.

## Consequences

**Positive**

- Claude Code can change its format and no data is lost; a parser fix plus re-projection recovers
  fields the old parser did not understand, retroactively, across all history.
- Re-projection needs no access to the original files, which may have rotated, moved (`relocated`
  records already exist in the corpus) or been deleted.
- The dedup hash makes recovery cheap: on file truncation or inode change, re-scan from zero and let
  the unique constraint discard what is already held.
- Raw bodies are the natural anchor for the Phase 7 integrity hash-chain, and the honest artifact to
  hand over when an audit trail is actually challenged.

**Negative**

- Storage roughly doubles: raw bodies plus projected columns. Mitigated by retention caps (Phase 7)
  and measured against the real corpus in Phase 1.
- Two-step ingestion is slightly more code and one more moving part than parsing straight into
  tables.

**Neutral**

- Re-projection must be cheap enough to run in-app. On a corpus this size it is a bounded scan.

## Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| Parse directly into normalized tables | Anything the parser does not understand is lost forever. Unacceptable when the source format demonstrably drifts across 12 versions in one user's history. |
| Keep raw only for unrecognized records | Requires the parser to correctly know what it does not know — precisely the assumption that fails when a *known* record type gains a new field. |
| Re-read the original files on schema change | Files rotate, move and are deleted; history would silently shrink over time. |
| Rigid typed schema per Claude Code version | A parser matrix to maintain forever, and it still fails on the first unreleased version. |
