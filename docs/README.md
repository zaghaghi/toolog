# Documentation

**Status: all nine phases complete — this is v1.0.** Capture works end to end, all four views are
built, and the record earns the word "audit": it says what it is missing, proves it has not been
altered, keeps secrets out of what it shows, bounds itself, and fails the build if anything tries
to leave the machine. It now also installs and uninstalls as one signed, notarized universal
artifact — and ships with **no network call at all**, the update check ADR-0008 had reserved
having been evaluated in Phase 8 and declined.

## Phases

Nine phases. Each ends in something runnable and checkable. **Phase 5 was the first usable release
(v0.1); Phase 8 is v1.0.**

| Phase | Goal | Milestone |
|---|---|---|
| [00 — Foundation](phases/00-foundation.md) | Repo, workspace, decision record | done |
| [01 — Storage core](phases/01-storage-core.md) | Schema migrates and round-trips | done |
| [02 — Transcript ingestion](phases/02-transcript-ingestion.md) | Backfill and live tail; the content lane | done |
| [03 — OTLP collector](phases/03-otlp-collector.md) | Decisions, durations, cost; the decision lane | done |
| [04 — App shell](phases/04-app-shell.md) | Installs itself, stays resident, frontend can query | done |
| [05 — Timeline view](phases/05-timeline-view.md) | The forensic view | **v0.1** — done |
| [06 — Risk, analytics & live](phases/06-risk-analytics-live.md) | The other three surfaces | done |
| [07 — Privacy, retention, integrity](phases/07-privacy-retention-integrity.md) | Earn the word "audit" | done |
| [08 — Packaging & distribution](phases/08-packaging-distribution.md) | Dead simple install | **v1.0** — done |

## Decisions

See [docs/adr/](adr/README.md) — nine ADRs, each with the alternative it rejected and why.

## Facts the design rests on

Verified against Claude Code **2.1.260** and a real 39-file transcript corpus. Re-check if the
version moves substantially.

| Fact | Consequence |
|---|---|
| OTEL `tool_input` truncates values at 512 chars, ~4 KB total | OTEL alone cannot hold evidence → [ADR-0002](adr/0002-dual-ingestion-transcripts-and-otel.md) |
| ~~Rejected tool calls leave no transcript record at all~~ — **disproved in Phase 4.** A refused call appears in both lanes; the transcript holds the `tool_use` block and a `tool_result` containing the refusal message | The conclusion stands on firmer ground: only OTEL carries `decision` and `decision_source`, so transcripts alone cannot answer *who* refused a call or *under which rule* → [ADR-0002](adr/0002-dual-ingestion-transcripts-and-otel.md), [ADR-0009](adr/0009-correlate-on-tool-use-id.md) |
| 12 Claude Code versions, 21 record types, `toolUseResult` as object/string/array | Store raw, project normalized → [ADR-0004](adr/0004-store-raw-project-normalized.md) |
| `tool_use_id` is present and exact in every lane | No heuristic matching → [ADR-0009](adr/0009-correlate-on-tool-use-id.md) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` is global across signals | Use per-signal variables only → [ADR-0006](adr/0006-configure-via-settings-env-block.md) |
| `~/.claude/settings.json` supports an `env` block | Install is one file write → [ADR-0006](adr/0006-configure-via-settings-env-block.md) |
| 271 of 2,334 local tool calls are `isSidechain` | Subagent attribution is not an edge case → [Phase 2](phases/02-transcript-ingestion.md) |
| Every one of 342 real `structuredPatch` hunks carries the same five keys — and four lines in them are `\ No newline at end of file`, which is a note rather than a line | The diff renderer counts it as neither → [Phase 5](phases/05-timeline-view.md) |
| 987 OTLP records across 11 event types carry **no** `permission_mode` attribute and no `permission_mode_changed` event; the transcript carries the mode in two shapes | The column belongs to the transcript lane → [Phase 6](phases/06-risk-analytics-live.md) |
| `prompt` and `response` attributes **are** present on OTLP records, with the literal value `<REDACTED>` | The privacy posture is measured, not assumed → [ADR-0008](adr/0008-local-only-zero-egress.md) |
| The window's CSP (`style-src 'self'`) silently discards `style` **attributes**; CSSOM assignment is honoured | Chart geometry goes through `element.style` → [Phase 6](phases/06-risk-analytics-live.md) |
| 48 of 3,506 stored results — 1.4% — hold 11 MB of the projection's 18 MB of result bodies | Bodies over 64 KiB are kept by reference → [Phase 7](phases/07-privacy-retention-integrity.md) |
| `session.transcript_path` is the only link between a projection row and its evidence | Retention removes whole sessions, both halves together → [Phase 7](phases/07-privacy-retention-integrity.md) |
| `plutil -lint` accepts an entitlements file that `codesign` rejects: XML forbids `--` inside a comment, and AMFI then signs the app **without** the entitlements rather than stopping | The plists are asserted by a test, not by a linter → [Phase 8](phases/08-packaging-distribution.md) |
| Tauri notarizes and staples the `.app` but only *signs* the `.dmg` around it | The `.dmg` is notarized in its own right, or the first double-click is a Gatekeeper warning → [Phase 8](phases/08-packaging-distribution.md) |

## Backfill numbers to check against

The owner's corpus, as of planning. Phase 2 is done when these reproduce:

```
39 transcript files, 42.2 MB, Claude Code 2.1.161 → 2.1.259
2,334 tool calls   Bash 1,539 · Edit 292 · Read 253 · Write 69
271 sidechain calls attributed to named agents
```

## Design mockups

Static pages that link the application's own stylesheets, so they cannot drift from it and the CSS
can be looked at in a browser without driving the window.

| Page | What it pins |
|---|---|
| [timeline.html](design/timeline.html) | Row anatomy, the detail pane, the states a list can be in (task 5.2) |
| [analytics.html](design/analytics.html) | Chart forms and mark geometry, at the numbers a real store produces (task 6.6) |

Both are worth opening after a change to `ui/src/styles/`. Neither replaces looking at the
application: a `file://` page has no Content Security Policy, and the window does — which is how a
chart came to render correctly in Chrome and not at all in the app.
