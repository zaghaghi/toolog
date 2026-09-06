# Phase 10 — One lens: the query bar, the histogram, and a pane that closes

**Goal:** the timeline becomes the only view over the store, so it has to read like one — one search
box that also filters, an activity histogram that both describes and sets the time range, and a
detail pane the reader can close.

**Depends on:** [Phase 9](09-subtraction.md). **Unblocks:** nothing.
**Governed by:** [ADR-0001](../adr/0001-tauri-2-for-the-desktop-shell.md),
[ADR-0003](../adr/0003-sqlite-as-the-embedded-store.md).

## Why

Three points from the owner's report on v1.0:

| Report | What it is |
|---|---|
| "Filters are too much, too many items to filter. I like the style of filtering that is embedded in the search edit box, like GitHub or Datadog." | Seven `<select>`s (`ui/src/filterbar.ts:168-256`) sitting above a search box that already exists. |
| "*Tool calls per day* … move it to the timeline, and adjust the time to the time of the timeline — something like AWS Log Insights." | The chart Phase 9 deleted was keyed on an analytics `Period`. On the timeline it must be keyed on the `TimelineFilter`, and it must set the range as well as show it. |
| "The detail panel should be closable." | It closes on `Escape`, but only while focus is in the list (`ui/src/timeline.ts:378`). There is no button, and in a narrow window the pane covers the list it was opened from. |

The syntax is **Datadog-style `@key:value`**, decided by the owner and confirmed by the code:
`crates/toolog-core/src/fts.rs:1-8` names `foo:bar` as an ordinary thing to search for in a corpus
that is 66% shell commands. A bare `key:value` would have been ambiguous with the search itself; the
sigil is not decoration.

## Tasks

### The activity histogram

- [x] **10.1** `query::histogram(conn, &TimelineFilter, bucket, utc_offset_minutes) -> Vec<Bucket>`
  in `crates/toolog-core/src/query.rs`, built on the existing private `selection()` (`query.rs:91`)
  so the histogram and the list can never describe different rows. **One measure — calls** — with
  failures and refusals carried per bucket for the tooltip only. Phase 6 settled that two scales on
  one chart let a reader see a correlation the data does not contain.
- [x] **10.2** Bucket size is derived from the span, not chosen by the reader: minute, hour, day or
  week, whichever puts the span in roughly sixty buckets. With no time bounds the span is the
  store's own first-to-last, from one `min/max(called_at)` — indexed by `tool_call_called_at`
  (`migrations/001_initial.sql:93`).
- [x] **10.3** The `timeline_histogram` command and the chart between the bar and the viewport,
  collapsible and remembered. Reuses `columnChart`, whose marks are sized through the CSSOM because
  the window's CSP silently discards a `style` **attribute**.
- [x] **10.4** Click a column to set the range to that bucket; drag across columns to brush a range.
  Both write **absolute** `since`/`until` into the same hash the list reads, so the chart, the list,
  the count and an export are the same filter by construction. A brushed range shows as "Custom
  range" in the time control, which already has that state (`ui/src/filterbar.ts:213`).

### One query bar

- [x] **10.5** `ui/src/query.ts`: `parse(text) -> { filter, terms, errors }` and
  `format(filter) -> text`. `@tool:Bash @project:toolog @outcome:refused rm -rf` narrows on the
  pairs and full-text-searches the rest. Round-trip is a test, not a hope: `parse(format(f))` equals
  `f` for every field of `TimelineFilter`.
- [x] **10.6** One key per control being deleted: `@project`, `@tool`, `@session`, `@agent`,
  `@source` (decision source), `@mode` (permission mode), `@decision`, `@outcome` (`ok` / `failed` /
  `refused`, over the `success`+`decision` collapse in `view.ts:56-69`), `@lane` (`view.ts:74-85`),
  `@thread` (`view.ts:90-98`), `@sidechain`. **Time stays a control**, because it pairs with the
  histogram.
- [x] **10.7** `TimelineFilter` stays the single source of truth and the hash keeps its current
  encoding (`ui/src/view.ts:105-121`). The query bar is a second *editor* of that filter, not a
  second representation of it — so a v1.0 link still restores, and Export still exports the filter
  rather than the text.
- [x] **10.8** Autocomplete: `@` offers the keys, `@key:` offers that key's values from the
  `facets()` call the timeline already makes (`ui/src/timeline.ts:145-151`). Keyboard-first — arrows,
  Enter, Escape. The values come from the store, not a hard-coded list, for the reason
  `filterbar.ts:8-11` already gives.
- [x] **10.9** Quoting: `@project:"/Users/me/some project"`. Facet values are real paths and real
  MCP tool names, so quotes are needed on day one and `format()` must add them wherever it
  round-trips one.
- [x] **10.10** An unrecognised key is an inline error under the box, naming the key and listing the
  valid ones; the rest of the query still applies. A half-typed token is not an error. Bare text
  keeps going through `fts::build_query`'s sanitisation.
- [x] **10.11** Delete the seven `<select>`s (`ui/src/filterbar.ts:168-256`). Time, Group by session
  and Export stay.

### A pane that closes

- [x] **10.12** A close button in the detail pane's header, `Escape` from anywhere inside the pane,
  and clicking the open row to close it. All three clear `selected` from the hash, so the back
  button undoes each.
- [x] **10.13** Update `docs/design/timeline.html`. It pins row anatomy, the pane and the states a
  list can be in, and now has a query bar, a histogram and a close button to pin too. It links the
  real stylesheets so it cannot drift — but a `file://` page has no CSP, so it does not replace
  looking at the window.

## Exit criteria

- Every filter reachable through a dropdown in v1.0 is reachable by typing, and a v1.0 hash link
  still restores its view.
- Dragging across the histogram narrows the list, and the resulting hash reproduces the view.
- The histogram over the owner's full store, with no time bounds, paints in under 200 ms — the same
  budget Phase 5 set for the list's first paint.
- The pane closes three ways, and the back button undoes each.
- `parse(format(f)) === f` holds for every field of `TimelineFilter`, including a project path with
  a space in it.

## Outcome

**Done.** `just check` is green: 144 frontend tests and the Rust suite. 1,316 lines added, 225
removed; the bundle goes from 50.09 kB of JavaScript to 62.54 kB and from 23.13 to 24.86 kB of CSS,
which is roughly what Phase 9 had just taken out of it.

### Measured, on the owner's real store

4,221 calls, 151 MB. `cargo run --release -p toolog-core --example measure_histogram -- <db>`,
median of five after a warm-up:

| Filter | Time | Result |
|---|---|---|
| No bounds — the whole store | **3.6 ms** | 14 columns of `Week` |
| One project | 0.1 ms | 50 columns of `Minute` |
| A full-text term (`cargo`) | 0.9 ms | 14 columns of `Week` |

Against the 200 ms Phase 5 set for the list's first paint, which this shares because it loads with
the list.

**One thing that did not go the way task 10.2 described.** It says the store's first-to-last comes
"from one `min/max(called_at)` — indexed by `tool_call_called_at`". It does not use that index:
`called_at_span` goes through `selection()` like everything else here, and `selection()`'s
`LEFT JOIN session` makes SQLite scan `tool_call_session` instead
(`EXPLAIN QUERY PLAN` confirms it). Timed separately on the real store: the joined query is 0.32 ms,
the plain one 0.11 ms, and the `GROUP BY` that actually builds the columns — which has to touch
every matching row whatever the span query does — is 0.50 ms. Two tenths of a millisecond is not
worth a second code path that could describe different rows from the list, which is the one property
task 10.1 exists to protect. Revisit if a store ever gets large enough for the scan to show.

### Decisions the tasks left open

- **`@outcome` does not clear `@decision`, and vice versa.** The dropdown it replaces did — choosing
  "Failed" cleared `decision`, which was invisible because a dropdown *is* the whole control. A
  token sitting beside `@decision:accept` in the same box is not invisible, and silently deleting
  the neighbour the reader can see would be a surprise. It also makes the round trip total: every
  `(success, decision)` pair the store can hold is sayable. `view.ts`'s `outcomeOf`/`withOutcome`
  had no caller left after that and went with the dropdown.
- **A leading `@` in free text is quoted, not escaped.** `format()` emits `"@reboot"` for a search
  term that starts with the sigil, and quoted values take `\"` — a project path can contain a quote
  and losing it silently would mean the box could not express a filter the store holds.
- **The Time control assigns `select.value` after appending its options** rather than marking one
  `selected` before insertion. The two are equivalent in a browser; they are not in every DOM, and
  the test for "a brushed range shows as Custom range" failed against the old pattern before it
  failed against anything real. The old `select()` helper had the same shape and no test.

### The exit criteria

- **Every v1.0 dropdown filter is typeable, and a v1.0 link still restores.** Both are tests, not
  claims: `query.test.ts` round-trips every field of `TimelineFilter` — one at a time, all at once,
  and starting from a hash copied out of a v1.0 window — and asserts every key in the language
  writes something back. `view.test.ts` restores that hash field by field.
- **Dragging narrows the list, and the hash reproduces the view.** A three-column drag in
  `timeline.test.ts`, with the chart given a width to measure against, asserts the absolute bounds
  and then `fromHash(toHash(view))`.
- **Under 200 ms on the full store.** 3.6 ms — table above.
- **The pane closes three ways, and the back button undoes each.** Button, `Escape` from inside the
  pane, and clicking the open row: three tests, all routed through one `closeDetail` that clears
  `selected` from the hash, which is what makes the back button work for all three.
- **`parse(format(f)) === f` for every field, including a path with a space.** Twenty-one cases,
  including `/tmp/a "quoted" dir` and `@reboot`.

### Not verified

The window was built and launched against the real store — it starts, binds the receiver and runs —
but **nobody has looked at it.** Every assertion above is a headless DOM or a Rust test. Three
things those cannot see: whether 78 px of chart is enough to read, whether the completion menu
lands somewhere sensible over a narrow window, and whether the brush band reads as a range rather
than as a highlight. `docs/design/timeline.html` pins the intended anatomy for all three, and it is
a `file://` page with no CSP — so it is not the last thing to look at either.
