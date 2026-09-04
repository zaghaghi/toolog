# Architecture Decision Records

Each ADR records one decision, why it was taken, what it costs, and what was rejected. They are
numbered in the order decided, not in dependency order.

| # | Decision | Status |
|---|---|---|
| [0001](0001-tauri-2-for-the-desktop-shell.md) | Tauri 2 for the desktop shell | Accepted |
| [0002](0002-dual-ingestion-transcripts-and-otel.md) | Dual ingestion: transcripts for content, OTEL for decisions | Accepted |
| [0003](0003-sqlite-as-the-embedded-store.md) | SQLite (rusqlite, bundled) as the embedded store | Accepted |
| [0004](0004-store-raw-project-normalized.md) | Store raw envelopes verbatim; normalization is a re-runnable projection | Accepted |
| [0005](0005-embedded-otlp-receiver.md) | Embed the OTLP receiver; never require the OpenTelemetry Collector | Accepted |
| [0006](0006-configure-via-settings-env-block.md) | Configure via the `settings.json` `env` block, per-signal variables only | Accepted |
| [0007](0007-single-resident-process.md) | One resident process, LaunchAgent-managed | Accepted |
| [0008](0008-local-only-zero-egress.md) | Local-only: loopback bind, zero egress, opt-in content capture | Accepted |
| [0009](0009-correlate-on-tool-use-id.md) | Correlate on `tool_use_id`; treat lane disagreement as a finding | Accepted |

## The two that drive everything else

**[ADR-0002](0002-dual-ingestion-transcripts-and-otel.md) — two lanes, not one.** OTEL truncates
tool inputs at 512 characters; transcripts have no permission decisions, no durations, no cost, and
no record of rejected calls at all. Neither source is a complete audit trail. Together they are, and
their disagreement is itself the completeness check ([ADR-0009](0009-correlate-on-tool-use-id.md)).

**[ADR-0004](0004-store-raw-project-normalized.md) — raw first, always.** One ordinary user's
history spans 12 Claude Code versions, 21 record types, and three different shapes of
`toolUseResult`. The formats drift. Data lost at ingestion cannot be recovered, so nothing is parsed
before it is stored.

## Open

- **SQLCipher / encryption at rest** — evaluated in [Phase 7](../phases/07-privacy-retention-integrity.md),
  to be recorded as an addendum to [ADR-0008](0008-local-only-zero-egress.md).
- **Hooks as a third ingestion lane** — rejected for v1 in
  [ADR-0002](0002-dual-ingestion-transcripts-and-otel.md); revisit only if Phase 7 reconciliation
  shows the transcript tail dropping calls.

## Format

Context (what forced the decision) → Decision → Consequences (positive, negative, neutral) →
Alternatives considered and rejected. A rejected alternative with no stated reason is not a record.

Superseding rather than editing keeps the history honest: mark the old ADR `Superseded by NNNN` and
write a new one.
