# Phase 9 — Subtraction: the usage and live views are removed

**Goal:** two of the four views do not earn their place. They and everything that existed only to
serve them leave, so Phases 10 and 11 have less to carry.

**Depends on:** nothing. **Unblocks:** Phases 10 and 11, both smaller once this lands.
**Governed by:** [ADR-0001](../adr/0001-tauri-2-for-the-desktop-shell.md),
[ADR-0002](../adr/0002-dual-ingestion-transcripts-and-otel.md), and a new ADR-0010 written here.

## Why

v1.0 shipped four views because the plan said four views. Hours of real use said otherwise. The
owner's report, and what each point is in the code:

| Report | What it is |
|---|---|
| "The usage page is useless to me — I don't want to know about costs in this application; there are tons of other applications that do the cost-related things." | `ui/src/analytics.ts` (501 lines) and the cost half of `toolog-core::analytics` (850 lines) answer a question the owner does not have. |
| "The only chart I found useful is *Tool calls per day*, which we could move to the timeline and adjust the time to the time of the timeline — something like AWS Log Insights." | That chart is keyed on an analytics `Period`, not on a `TimelineFilter`. Re-keying it is [Phase 10](10-one-lens.md). |
| "The live page is much like the timeline, the same kind of information; better to be removed." | It is: the same rows, pushed rather than queried, without filters, search, grouping or a detail pane. |

Three decisions were taken before this phase was written, and are not reopened here:

- Cost analytics go **entirely** — the view, the command, the CLI subcommand and the cost/token half
  of `toolog-core::analytics`. `api_request` keeps being ingested; nothing displays it.
- The live view is **deleted outright**. Only the existing "N new calls" pill survives, and the two
  notification switches move to Status.
- A completed phase document is a record, not a specification. Nothing here edits Phases 0–8.

## Tasks

- [x] **9.1** Delete the usage screen: `ui/src/analytics.ts`, `ui/src/analytics.test.ts`, its entry
  in `SCREENS` and its case in `show()` (`ui/src/main.ts:32-40`, `:139-150`).
- [x] **9.2** Delete the live screen: `ui/src/live.ts`, `ui/src/live.test.ts`, its tab, and the
  `live.noteCall` half of the `live_tool_call` listener (`ui/src/main.ts:171-174`). The event keeps
  feeding `timeline.noteLiveCall`; the "N new calls" pill is the whole of what survives.
- [x] **9.3** Move the two notification switches (`notify_refusals`, `notify_high_risk`) to the
  Status tab, beside the `redact_evidence` switch already there (`ui/src/setup.ts:340-370`). Same
  `Prefs` round-trip, same defaults — both off.
- [x] **9.4** Remove the backend that only those two views used: the `usage` and `live_sessions`
  commands (`crates/toolog-app/src/commands.rs:502-519`), the `toolog usage` subcommand
  (`crates/toolog-cli/src/cli.rs:224-230`, `commands.rs:601-628`, `render_usage`,
  `render_breakdowns`, `money`), and from `crates/toolog-core/src/analytics.rs`: `Period`, `bounds`,
  `breakdown`, `call_stats`, `cost_stats`, `coverage`, `percentiles`, `active_ms`, `by_day`,
  `by_project`, `by_model`, `by_session`, `tools`, `headline`, `compare`, `live_sessions`. With them
  goes `crates/toolog-core/tests/analytics.rs`.
- [x] **9.5** Keep `local_day()` (`analytics.rs:115-120`) by moving it to
  `crates/toolog-core/src/query.rs` **with its comment intact**. "Days are the reader's days" is the
  one piece of that module Phase 10 still needs, and the reasoning is worth more than the four lines.
- [x] **9.6** Trim `ui/src/chart.ts` to what Phase 10's histogram needs — `columnChart`, `ticks`,
  `scaleTop`, `figure`/`tableTwin` — deleting `barChart`, `dataTable`, `meter`, `statTile`,
  `sparkline` and their cases in `chart.test.ts`. **Keep the CSP test** (`chart.test.ts:224-257`): no
  mark may ever carry a `style` attribute, which is the trap that cost Phase 6 a day. Drop `cost()`
  from `ui/src/format.ts` once nothing calls it.
- [x] **9.7** Update `crates/toolog-cli/tests/egress.rs:68-71`. It runs *every query the window
  issues* and then asks the operating system which sockets the process holds; it currently calls
  `analytics::analytics`, `analytics::compare` and `analytics::live_sessions`. The zero-egress
  guarantee is only as good as that list, so it gets the **new** list, not a shorter one.
- [x] **9.8** Delete the chart and live blocks of `ui/src/styles/app.css` (~`:952-1401`, ~`:1749+`),
  keeping what the risk table borrows — `.chart-table`, `.chart-titles`, `.chart-title`,
  `.chart-caption` — and delete `docs/design/analytics.html`.
- [x] **9.9** **ADR-0010 — toolog does not report cost.** A scope decision, not a tidy-up: ADR-0002
  justified the OTEL lane partly by the cost it carries, and without a record someone will
  reasonably re-add a usage view in a year. The ADR keeps ADR-0002 standing on `decision` and
  `decision_source`, which nothing else can supply; says `api_request` is still captured as
  evidence; and names the rejected alternative, a usage view stripped of cost.
- [x] **9.10** Regenerate the boundary (`just bindings`) and correct every place that promises four
  views: `README.md:3`, `:23` and the diagram at `:54`; `PRIVACY.md:124`, which points notifications
  at "the Live tab"; `docs/README.md:3` and its mockup table; `ui/package.json:6`; and the two ADRs
  that reason from the count, `0001:14,:40` and `0003:18`. **Amend, do not rewrite:** an ADR records
  what was decided and why, and "three views justify a framework even less" is a footnote on 0001,
  not a new decision.

## Exit criteria

- `just check` is green, with no `#[allow(dead_code)]` added to get there.
- The window has three tabs: Timeline, Risk, Status.
- `rg -n 'usage|live_sessions|cost_usd' ui/src crates/toolog-app/src` finds nothing.
- `toolog --help` no longer lists `usage`. `toolog verify` and `toolog risk` are unaffected, and
  `toolog verify` still counts `api_request` rows — the lane is still captured, just not displayed.
- Notifications still fire, still default off, and still survive a restart — now switched from
  Status.

## Outcome

**Done.** The window has three tabs. `just check` is green — 95 frontend tests and the Rust suite,
with no `#[allow(dead_code)]` anywhere in the diff.

What left, measured rather than estimated:

| | Added | Deleted |
|---|---|---|
| Rust and TypeScript (`crates/`, `ui/`) | 265 | 4,217 |
| of which `ui/src/styles/app.css` | 10 | 372 |

The built bundle went from 72.33 kB of JavaScript and 27.71 kB of CSS to **50.09 kB and 23.13 kB** —
a third of the script gone. Two Rust files (`analytics.rs`, `tests/analytics.rs`), four TypeScript
files (`analytics.ts`, `live.ts` and both test files) and one mockup page were deleted outright.

### Three things went further than the task list said, and one thing did not

- **The `stats` command went too** (9.4). It is not in the task list, but its own doc comment read
  "Everything the analytics view opens with", nothing in the frontend has ever called it, and the
  exit criterion's `rg` matches its `query::stats_tool_usage` call. `Stats`, and with it `Totals`,
  `ToolUsage` and `Reconciliation`, left the generated bindings. The core functions stay — `toolog
  doctor` and the capture status read `stats_totals`.
- **`SessionGroup.cost_usd_micros` went too** (9.4). The timeline's session headers rendered it
  ("no cost recorded", or a figure), which is the decision's "nothing displays it" and the exit
  criterion's `rg cost_usd` over `ui/src`. The field, the query that filled it and the header text
  are gone, which is what finally made `cost()` unused in `format.ts` as 9.6 anticipated.
- **The column chart's CSS stayed** (9.8). The task named the block at `:952-1401` and the four
  selectors the risk table borrows. Taken literally that also deletes the styling for the one chart
  form 9.6 keeps, leaving Phase 10 to re-draw it. What was deleted instead is every form that lost
  its code — bars, the table card, meters, stat tiles, sparklines — and the whole live/feed block.
  `.switch` and the uninstall block sat under the live banner and are Status furniture now, so they
  were re-bannered rather than moved.
- **`local_day` is `pub`, not private** (9.5). Nothing in `query.rs` calls it yet — Phase 10's
  histogram will — and a private helper with no caller is a dead-code warning. `#[allow(dead_code)]`
  was the thing the exit criteria forbade, so the function is public with the reason written down.

  > **Corrected in Phase 10.** The histogram did not need it and the function is now deleted. It
  > returns a `date()` string; the chart needs a bucket *start in milliseconds*, for every one of
  > four bucket sizes, and integer arithmetic on the shifted timestamp gives that in one expression
  > where a date string would have needed converting back. What was worth keeping was the
  > reasoning, not the four lines — "days are the reader's days" is now the doc comment on
  > `query::histogram`'s `utc_offset_minutes`, and `days_are_the_readers_days` in
  > `crates/toolog-core/tests/timeline.rs` asserts it.

### The exit criteria

- `just check` green, no `#[allow(dead_code)]` in the diff. ✓
- Three tabs — Timeline, Risk, Status — asserted in `ui/src/main.test.ts` rather than only observed. ✓
- `rg -n 'usage|live_sessions|cost_usd' ui/src crates/toolog-app/src` returns **one** line:
  `risk.test.ts:156`, a fixture using the built-in rule id `mcp-tool-usage`
  (`crates/toolog-core/src/rules/default.toml:222`). It predates this phase and has nothing to do
  with the deleted views. Nothing else matches.
- `toolog --help` no longer lists `usage`; `verify` and `risk` are unaffected. ✓
- **`toolog verify` now counts `api_request` rows** — it did not before, here or in v1.0, so the
  criterion was made true rather than checked. `Completeness` carries an `api_requests` count and
  the report prints `API requests  N  the OTLP lane's other half, captured and not displayed`. A
  count of records is capture accounting, not cost reporting, which is what ADR-0010 point 2
  reserves. Measured on a store fed two `api_request` records and one refusal through the OTLP
  path: verify counted 2, the table held 2, and 19,134 micro-dollars sat in it unread.
- Notifications: the Rust side was not touched. The switches moved in the UI only, over the same
  `Prefs` round-trip, and `setup.test.ts` now carries the assertions the deleted `live.test.ts`
  held — both off on arrival, the whole `Prefs` sent on a change, and a failed save reverting the
  box.

### What Phase 10 inherits

`columnChart`, `ticks`, `scaleTop`, `figure`/`tableTwin` and their CSS; `query::local_day` with its
reasoning (which Phase 10 replaced — see the correction above); and `chart.test.ts`'s two surviving
guards — the CSP case that cost Phase 6 a day, and
the one asserting a chart label from a transcript is text rather than markup (re-pointed from the
deleted `barChart` at the column chart's x-ticks).

ADR-0010 records why none of this comes back.
