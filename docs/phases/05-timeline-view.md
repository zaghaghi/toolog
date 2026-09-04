# Phase 5 — Timeline view → **v0.1**

**Goal:** the forensic view — "what did the agent do to my repo last Tuesday?" First genuinely
usable build.

**Depends on:** Phases 1, 2, 4. **Unblocks:** Phase 6 (shared UI infrastructure).
**Governed by:** [ADR-0001](../adr/0001-tauri-2-for-the-desktop-shell.md),
[ADR-0003](../adr/0003-sqlite-as-the-embedded-store.md).

## Tasks

- [ ] **5.1** Frontend baseline: TypeScript + Vite, **no heavy component framework** (ADR-0001) —
  four views over a table do not justify one, and the bundle stays honest.
- [ ] **5.2** **Design pass before building rows.** Typography, density, spacing scale, and a
  light/dark palette driven by tokens. "Simple but beautiful" is an explicit requirement; retrofitting
  it onto a built table costs more than doing it first. Deliverable: a token file and one static
  mockup screen agreed before 5.3.
- [ ] **5.3** Virtualized list over `tool_call`, newest first. Target: smooth scroll at 100k rows,
  first paint under 200 ms. Server-side paging through `timeline_page` — never load the table into
  the WebView.
- [ ] **5.4** Row anatomy: time, project/session, tool badge, `input_summary` (the display line),
  duration, success/failure, decision-source chip. `Bash` is 66% of all calls locally, so the row
  must read well for a shell command above all else.
- [ ] **5.5** **Partial-row rendering.** Per ADR-0009, rows arrive half-populated between lanes.
  Missing duration or pending decision renders gracefully — never a blank row, never a hidden one.
- [ ] **5.6** Filters: time range, project, session, tool, success/failure, `is_sidechain`,
  decision source, permission mode. Filter state in the URL hash so a view is shareable and
  restorable.
- [ ] **5.7** FTS search across commands, paths and result text, with match highlighting.
  **Sanitize FTS5 operators** — `|`, `*`, `"`, `-`, `:` and `NEAR` are everywhere in shell commands
  and will otherwise produce syntax errors or silently wrong results.
- [ ] **5.8** Detail pane: full `input_json`, full result, envelope metadata (cwd, git branch,
  Claude Code version, permission mode), and provenance — which lanes witnessed this call.
- [ ] **5.9** **Diff rendering for `Edit`/`Write` from `structuredPatch`** (354 rows locally).
  The most information-dense thing this app can show; worth doing properly rather than as raw JSON.
- [ ] **5.10** Session grouping with a collapsible header: project, branch, duration, tool count,
  cost. Subagent calls nested under their `agent-name` rather than flattened into the main thread.
- [ ] **5.11** Jump-to-source: open the underlying transcript at the right line.
- [ ] **5.12** Export the current filter → JSON / CSV / Markdown evidence bundle, including the
  provenance of every exported row.
- [ ] **5.13** Empty, loading and error states, including "collector is not running" with a link to
  `doctor` output.

## Exit criteria

- On the backfilled corpus: find every `rm -rf` ever run, narrow to one project, inspect one, and
  export the set — in under a minute, without touching a terminal.
- Scrolling the full corpus stays smooth; search returns in under 200 ms.
- An `Edit` row shows a readable diff.
- The app is genuinely useful at this point. **This is v0.1** — ship it before starting Phase 6.
