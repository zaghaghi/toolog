# Phase 5 — Timeline view → **v0.1**

**Goal:** the forensic view — "what did the agent do to my repo last Tuesday?" First genuinely
usable build.

**Depends on:** Phases 1, 2, 4. **Unblocks:** Phase 6 (shared UI infrastructure).
**Governed by:** [ADR-0001](../adr/0001-tauri-2-for-the-desktop-shell.md),
[ADR-0003](../adr/0003-sqlite-as-the-embedded-store.md).

## Tasks

- [x] **5.1** Frontend baseline: TypeScript + Vite, **no heavy component framework** (ADR-0001) —
  four views over a table do not justify one, and the bundle stays honest.
- [x] **5.2** **Design pass before building rows.** Typography, density, spacing scale, and a
  light/dark palette driven by tokens. "Simple but beautiful" is an explicit requirement; retrofitting
  it onto a built table costs more than doing it first. Deliverable: a token file and one static
  mockup screen agreed before 5.3.
- [x] **5.3** Virtualized list over `tool_call`, newest first. Target: smooth scroll at 100k rows,
  first paint under 200 ms. Server-side paging through `timeline_page` — never load the table into
  the WebView.
- [x] **5.4** Row anatomy: time, project/session, tool badge, `input_summary` (the display line),
  duration, success/failure, decision-source chip. `Bash` is 66% of all calls locally, so the row
  must read well for a shell command above all else.
- [x] **5.5** **Partial-row rendering.** Per ADR-0009, rows arrive half-populated between lanes.
  Missing duration or pending decision renders gracefully — never a blank row, never a hidden one.
- [x] **5.6** Filters: time range, project, session, tool, success/failure, `is_sidechain`,
  decision source, permission mode. Filter state in the URL hash so a view is shareable and
  restorable.
- [x] **5.7** FTS search across commands, paths and result text, with match highlighting.
  **Sanitize FTS5 operators** — `|`, `*`, `"`, `-`, `:` and `NEAR` are everywhere in shell commands
  and will otherwise produce syntax errors or silently wrong results.
- [x] **5.8** Detail pane: full `input_json`, full result, envelope metadata (cwd, git branch,
  Claude Code version, permission mode), and provenance — which lanes witnessed this call.
- [x] **5.9** **Diff rendering for `Edit`/`Write` from `structuredPatch`** (354 rows locally).
  The most information-dense thing this app can show; worth doing properly rather than as raw JSON.
- [x] **5.10** Session grouping with a collapsible header: project, branch, duration, tool count,
  cost. Subagent calls nested under their `agent-name` rather than flattened into the main thread.
- [x] **5.11** Jump-to-source: open the underlying transcript at the right line.
- [x] **5.12** Export the current filter → JSON / CSV / Markdown evidence bundle, including the
  provenance of every exported row.
- [x] **5.13** Empty, loading and error states, including "collector is not running" with a link to
  `doctor` output.

## Exit criteria

- [x] On the backfilled corpus: find every `rm -rf` ever run, narrow to one project, inspect one,
  and export the set — in under a minute, without touching a terminal. *Every step is a tested code
  path and a measured query (43 matches in 1.7 ms, 0.22 ms once narrowed to a project); the flow was
  not walked by a person in the window — see **Not verified**.*
- [x] Search returns in under 200 ms: 24 ms at 100k rows for a term matching every one of them,
  1.7 ms on the real store. Scrolling holds no measured stall — no query on the path exceeds 43 ms —
  but smoothness itself was not observed.
- [x] An `Edit` row shows a readable diff, rendered from the shape all 342 hunks in the real store
  actually have.
- [ ] The app is genuinely useful at this point. **This is v0.1** — ship it before starting Phase 6.
  *Left unticked deliberately: this is a judgement about using the thing, and it is the owner's to
  make. Everything it depends on is done and measured below.*

## Outcome

191 Rust tests and 64 frontend tests, `just lint` clean. The window is now TypeScript compiled by
Vite and embedded in the binary; CI gained a Node step that type-checks, tests and bundles it before
anything compiles.

### The design pass, and what it settled

Task 5.2's deliverable is [`ui/src/styles/tokens.css`](../../ui/src/styles/tokens.css) and
[`docs/design/timeline.html`](../design/timeline.html) — a static mockup that links the *same* token
file the application uses, so the two cannot drift apart. Two things were agreed there before a row
was written:

**Dense single-line rows.** `Bash` is 71% of the owner's corpus, and the question asked of it is
almost always "what ran, and did it go through?". A row is 28 px, showing time, project, tool,
outcome, the command in monospace, duration and decision source. Status is carried by a glyph rather
than by a coloured row, so a screen full of failures still reads as a list.

**A right-hand detail pane, and only while something is open.** The list keeps its place while the
pane follows the selection, so arrowing down a run of related calls inspects each one without a
second keystroke — and with nothing selected the pane is gone entirely rather than spending 440px on
a placeholder telling you to select something. Below 900px it covers the list instead of sitting
beside it.

One height for every item — rows, session headers and agent headers alike — is what makes "which
item is at pixel N?" a division. That is the whole basis of task 5.3, and it was a design decision
before it was an implementation one.

### Measured, not asserted

`cargo run --release -p toolog-core --example measure_timeline` builds a synthetic 100k-call store
across 400 sessions and times every query the view leans on. The target is 200 ms.

| Query | 100k synthetic | Real corpus (2,939 calls) |
|---|---|---|
| `timeline_count` (all) | 6.7 ms | 0.44 ms |
| First page (200 rows) | **0.34 ms** | 1.9 ms |
| Page at offset 90,000 | 8.6 ms | — |
| `timeline_groups` (all sessions) | 43 ms | 5.6 ms |
| Search, term matching **every** row | **24 ms** | — |
| Search, term matching 1 row in 9 | 10 ms | — |
| Search `rm -rf` | — | 1.7 ms |
| Search + project filter | 29 ms | 0.22 ms |
| `facets` | 17 ms | 2.4 ms |

**The first version of the page query missed the target, and the measurement is why it is not still
missing it.** Ordering by time means sorting every matching row, and SQLite was carrying fully-built
rows — 27 columns, a session join and two correlated subqueries — through that sort. A search whose
term appeared in all 100k rows took **250 ms**; a page at offset 90,000 took **65 ms**.

`timeline_rows` now picks the page in a subquery that selects nothing but `rowid`, and decorates only
the 200 rows that won. Sorting 100k integers and building 200 rows is the same answer: **24 ms** and
**8.6 ms**. It reads as one query too many, and the comment above it says to measure before
simplifying it back.

### A bug the phase's own filter work exposed

`toolog export --rejected` did not export refusals. `rejected_only()` filtered on
`provenance_mask = OTLP`, an inference Phase 4 had already disproved — a refused call appears in
*both* lanes, and `decision` is the only thing that identifies one. The mask matched every call OTEL
had witnessed: **30 rows on the owner's store, of which 2 were actually refused.**

Phase 4 corrected the ADRs and added `Reconciliation.rejected` reading from `decision`, but left this
filter on the old inference. It now filters on `decision = 'reject'`, with a test that seeds one
accepted and one refused call — both carrying the OTLP bit — and asserts the export contains one row.

`TimelineFilter.provenance_mask` was replaced by `provenance`, an exact match, in the same change. A
mask can say "the OTLP lane saw this" but cannot say "and the transcript lane did not", which is what
the lane filter in the UI actually needs — and it was a mask that made the wrong filter look
plausible for two phases.

### The query surface Phase 6 inherits

- **Search is a filter, not a mode.** `TimelineFilter` gained a `query` field that goes through
  `fts::build_query`, so a search composes with a project and a date range instead of being a second
  list that disagrees with the first. The `search` IPC command was removed; `query::search` survives
  as a thin wrapper over the same path for the CLI and the existing tests.
- **A row arrives ready to draw.** `TimelineRow` carries the call, its session's project and branch,
  the diff size, and — when the page was searched — the `snippet()` showing where the match was. At
  100k rows a second round trip per row is the difference between a list that scrolls and one that
  stutters.
- **`timeline_groups` is an index, not a page.** It returns one entry per session with its size, its
  subagents and their sizes, so a grouped list can compute its own height and collapse a session it
  has never fetched.
- **Cost is a property of the session, not of the filtered slice.** Narrowing to one tool must not
  make a session look cheaper than it was, and a session OTEL never saw reports `None` rather than
  zero — imported history has no cost data at all, and a zero reads as free.

Three filter fields exist because a query has to be able to say what it means: `main_thread`
partitions on `agent_id` (present on every sidechain call and no main-thread one) rather than on
`is_sidechain`, which is null on any row the transcript lane has not witnessed; `session_unknown`
names the group of calls whose session was never learned, without which "expand that group" would
quietly widen to every row; and `agent_id` scopes a subagent's block.

### Frontend tests, and what they are for

The window is 25 modules and about 38 kB of JavaScript. Its tests run under `vitest` with
`happy-dom` — nothing here measures layout, because the virtual list is deliberately arithmetic — and
they assert the phase's own sentences:

- a row says what ran, where, and how it went;
- a call only the transcript saw shows `—` with a title saying which lane is missing, not a zero;
- a call with no result yet is **shown**, not hidden;
- a refusal reads as a refusal, with who refused it;
- a search marks its term, and says so when the match was in a result the row does not show;
- a subagent's calls sit under their own header, and collapsing a session hides its rows;
- an empty store, a filter matching nothing, a stopped collector and a page that failed to load
  each say something useful, and the failed page can be asked for again without reloading the view;
- the window mounts — the one test that would catch "the window is blank", which is how a bundled
  frontend fails.

`Plan` is tested separately over its index arithmetic, including a 400-session, 100k-row plan, because
an off-by-one there shows a call under the wrong agent, which in this application is not a cosmetic
bug.

### The diff renderer was written against the real hunks, and one of them was a trap

All 299 non-empty `structuredPatch` values in the owner's store parse, and all 342 hunks in them
carry exactly the five keys the renderer expects. The line markers are `+` (3,164), a space (2,423),
`-` (1,336) — **and `\` four times.**

`\ No newline at end of file` is a note about the line above it, not a line of the file. The first
version of the renderer fell through to the context branch and advanced both line counters, shifting
every number after it in that hunk by one. Four occurrences, and each one silently mislabels the
lines below it in a view whose entire purpose is saying which line changed. Handled, and pinned by a
test.

### Deviations from the plan

**Jump-to-source reveals rather than opens (5.11).** The task says "open the underlying transcript at
the right line". The pane finds the stored record, shows `path:line`, prints the transcript line
itself, and offers *Reveal in Finder* — the one filesystem capability the window is granted. Opening
it would mean either launching the user's editor (which the app cannot know, and which the capability
list deliberately does not permit) or handing a 40 MB JSONL file to whatever owns `.jsonl`. Showing
the record inline is also the more honest artifact: the stored line is the evidence, and the file is a
convenience that may since have rotated.

**Session grouping is a mode, and subagents are their own groups (5.10).** Grouping reorders, and
newest-first is the right default for a forensic list, so it is a toggle rather than always on. Within
a session a subagent's calls form their own collapsible group under its `agent_name` — literally
nested, at the cost of moving them out of strict time order inside that session. Ungrouped, a
subagent's rows stay in time order and are indented with an agent chip instead.

**Session filtering is in the detail pane, not the filter bar (5.6).** A dropdown of every session id
is not a control anyone can use. The pane offers *Only this session*, *Only this project* and — for a
sidechain call — *Only this subagent*, which is how the filter is actually reached; the hash carries
`session=` either way, so a link still restores it.

**`Format` gained `Markdown`, so the CLI did too.** The export format is shared with
`toolog export --format markdown` rather than being a UI-only path. CSV and Markdown both carry a
`lanes` column now — an export that does not say how completely each row was observed is worth less
than one that does.

**A native save panel, and one new dependency.** `tauri-plugin-dialog` is used from Rust only, so no
new capability is granted to the WebView. It is the only path by which this process writes a file
outside its own store, and the path comes from the panel rather than from the window.

**`withGlobalTauri` is off.** The frontend now imports `@tauri-apps/api` properly, so the global
bridge Phase 4 leaned on is no longer injected.

### Not verified

**Nobody has looked at the window in this environment.** `screencapture` returns black without screen
recording permission, and the macOS window cannot be driven from here. Everything the frontend does
is exercised against a real DOM in tests, and the application builds, launches and serves its
commands — but "simple but beautiful" is a judgement that needs eyes on it, and the last exit
criterion is left unticked for the same reason.

**The export save panel was not driven end to end.** `save_export` opens a native panel, which cannot
be dismissed or confirmed from this environment. The serialization behind it is tested; the panel is
not.
