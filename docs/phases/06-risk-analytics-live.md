# Phase 6 — Risk, analytics & live views

**Goal:** the remaining three of the four surfaces the owner asked for, built on Phase 5's
infrastructure.

**Depends on:** Phases 3, 4, 5. **Unblocks:** Phase 7 (rules inform redaction).

## Risk & permission review

*"What got approved, and how?"* — the view only the OTEL lane makes possible (ADR-0002).

- [ ] **6.1** Rule engine over `tool_call`. **Rules as data, not code**, so new rules ship without a
  release and users can add their own.
- [ ] **6.2** Starter rule set:
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

## Exit criteria

- The risk view flags a deliberately auto-approved `rm -rf` in a scratch directory, with drill-through
  to the exact call.
- Analytics totals reconcile against `claude_code.api_request` costs for a live session.
- Two concurrent Claude Code sessions appear as separate live lanes with correct attribution.
