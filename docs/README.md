# Documentation

**Status: planning complete, implementation not started.**

## Phases

Nine phases. Each ends in something runnable and checkable. **Phase 5 is the first usable release
(v0.1); Phase 8 is v1.0.**

| Phase | Goal | Milestone |
|---|---|---|
| [00 — Foundation](phases/00-foundation.md) | Repo, workspace, decision record | |
| [01 — Storage core](phases/01-storage-core.md) | Schema migrates and round-trips | |
| [02 — Transcript ingestion](phases/02-transcript-ingestion.md) | Backfill and live tail; the content lane | |
| [03 — OTLP collector](phases/03-otlp-collector.md) | Decisions, durations, cost; the decision lane | |
| [04 — App shell](phases/04-app-shell.md) | Installs itself, stays resident, frontend can query | |
| [05 — Timeline view](phases/05-timeline-view.md) | The forensic view | **v0.1** |
| [06 — Risk, analytics & live](phases/06-risk-analytics-live.md) | The other three surfaces | |
| [07 — Privacy, retention, integrity](phases/07-privacy-retention-integrity.md) | Earn the word "audit" | |
| [08 — Packaging & distribution](phases/08-packaging-distribution.md) | Dead simple install | **v1.0** |

## Decisions

See [docs/adr/](adr/README.md) — nine ADRs, each with the alternative it rejected and why.

## Facts the design rests on

Verified against Claude Code **2.1.260** and a real 39-file transcript corpus. Re-check if the
version moves substantially.

| Fact | Consequence |
|---|---|
| OTEL `tool_input` truncates values at 512 chars, ~4 KB total | OTEL alone cannot hold evidence → [ADR-0002](adr/0002-dual-ingestion-transcripts-and-otel.md) |
| Rejected tool calls leave **no transcript record at all** | Transcripts alone cannot hold the permission story → [ADR-0002](adr/0002-dual-ingestion-transcripts-and-otel.md) |
| 12 Claude Code versions, 21 record types, `toolUseResult` as object/string/array | Store raw, project normalized → [ADR-0004](adr/0004-store-raw-project-normalized.md) |
| `tool_use_id` is present and exact in every lane | No heuristic matching → [ADR-0009](adr/0009-correlate-on-tool-use-id.md) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` is global across signals | Use per-signal variables only → [ADR-0006](adr/0006-configure-via-settings-env-block.md) |
| `~/.claude/settings.json` supports an `env` block | Install is one file write → [ADR-0006](adr/0006-configure-via-settings-env-block.md) |
| 271 of 2,334 local tool calls are `isSidechain` | Subagent attribution is not an edge case → [Phase 2](phases/02-transcript-ingestion.md) |

## Backfill numbers to check against

The owner's corpus, as of planning. Phase 2 is done when these reproduce:

```
39 transcript files, 42.2 MB, Claude Code 2.1.161 → 2.1.259
2,334 tool calls   Bash 1,539 · Edit 292 · Read 253 · Write 69
271 sidechain calls attributed to named agents
```
