# Documentation

**Status: v1.0 shipped, and Phase 9 has landed on top of it.** Capture works end to end and the
record earns the word "audit": it says what it is missing, proves it has not been altered, keeps
secrets out of what it shows, bounds itself, and fails the build if anything tries to leave the
machine. It installs and uninstalls as one signed, notarized universal artifact — and ships with
**no network call at all**, the update check ADR-0008 had reserved having been evaluated in Phase 8
and declined.

**v1.1 is in progress.** Living with v1.0 found four of its views to be two too many, and the risk
review — the one worth keeping — to be slow and self-contradictory. Phase 9 took the two views away
and Phase 10 rebuilt the timeline around one query bar and an activity histogram; Phase 11 is
planned and not started.

## Phases

Nine phases shipped v1.0. Three more are the revision after living with it, of which **Phases 9 and
10 are done and Phase 11 is not started**. Each phase ends in something runnable and checkable. **Phase 5 was
the first usable release (v0.1); Phase 8 is v1.0; Phase 11 is v1.1.**

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
| [09 — Subtraction](phases/09-subtraction.md) | The usage and live views are removed | done |
| [10 — One lens](phases/10-one-lens.md) | The timeline's query bar, histogram and closable pane | done |
| [11 — Risk, fast and legible](phases/11-risk-fast-and-legible.md) | A review that is fast, adds up, and can be read | **v1.1** — planned |

Phases 9–11 come from the owner's report after using v1.0: two views did not earn their place, the
timeline asks the reader to work too hard, and the risk view — the one that is good — is slow, does
not reconcile with itself, and never shows what its rules actually look for. Phase 9 stated the
decisions those three phases rest on and removed the two views; the window now has three tabs —
Timeline, Risk, Status. Phase 10 made the timeline the one lens over the store: seven dropdowns
became one `@key:value` box, and the one chart worth keeping from the usage view came back re-keyed
onto the timeline's own filter.

## Releasing

[releasing.md](releasing.md) — the six signing secrets, the tag that triggers everything, the tap,
and the clean-machine check that is the only real proof of the Phase 8 exit criterion.

## Decisions

See [docs/adr/](adr/README.md) — ten ADRs, each with the alternative it rejected and why.

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
| `foo:bar` is an ordinary thing to search for in a corpus that is two-thirds shell commands | The query bar's filter syntax needs a sigil: `@key:value`, not `key:value` → [Phase 10](phases/10-one-lens.md) |
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
| [timeline.html](design/timeline.html) | Row anatomy, the query bar and its completions, the activity histogram, the detail pane and its close button, and the states a list can be in (tasks 5.2, 10.13) |

`analytics.html` pinned the chart forms and their mark geometry; it went with the view it described
(task 9.8). The column chart it also pinned survives in `ui/src/chart.ts`, and task 10.13 put it
back on the timeline mockup where it is now drawn.

The page is worth opening after a change to `ui/src/styles/`. It does not replace looking at the
application: a `file://` page has no Content Security Policy, and the window does — which is how a
chart came to render correctly in Chrome and not at all in the app.
