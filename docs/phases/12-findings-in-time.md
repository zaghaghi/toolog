# Phase 12 — Findings in time, and risk as a filter

**Goal:** two things the review still cannot answer. *When did we first see this?* — currently
unanswerable, because a finding carries the call's timestamps and never its own. And *show me the
risky calls in the timeline* — currently impossible, because a rule's conditions have no equivalent
in a `TimelineFilter`.

**Depends on:** [Phase 11](11-risk-fast-and-legible.md), whose grouped per-rule query and
per-severity `OR` are what both halves are built on. **Unblocks:** nothing.
**Governed by:** [ADR-0004](../adr/0004-store-raw-project-normalized.md),
[ADR-0011](../adr/0011-memoize-the-risk-review.md), and a new ADR-0012 written here.

## Why

| The gap | What it is |
|---|---|
| "When did this start?" has no answer. | `Finding.first_at` and `last_at` are `min`/`max` of `tool_call.called_at` (`rules.rs:496`) — when the **call** ran, not when a rule caught it. Add a rule today and a three-month-old call reads as three months old. Nothing distinguishes *old news* from *new to me*. |
| A review has no history. | Findings are recomputed and nothing remembers what the last one said, so "what changed since Tuesday?" cannot be asked. Notifications (task 6.12) fire per **arriving call**, never per new finding. |
| A purge erases that a call was ever flagged. | `retention.rs:283-302` deletes the calls and sessions; the finding goes with them, because the finding was only ever computed from them. The `deletion` table survives what it describes (migration 005); nothing on the risk side does. |
| The timeline cannot show risky calls. | `rules::calls` exists precisely because it could not: "a rule's conditions have no equivalent in `TimelineFilter` — `outside_cwd` and `first_line` are not columns" (`rules.rs:718`). So the risk view has a private paging path that the timeline, the histogram and Export cannot reach. |

### Three decisions taken before this phase was written

- **Findings are still not stored.** ADR-0004 stands and ADR-0011 restated it: a `finding` table
  goes stale on every *ingest*, not only on a rule change, and a row saying "rule X matched call Y"
  becomes a claim about a rule that no longer exists the moment X is retuned. Phase 11 removed the
  only argument for storing them — a full re-analysis is **95 ms** on the owner's store.
- **What is stored is a *sighting*, not a finding.** `(rule_id, tool_use_id, first_seen)`,
  append-only. It never claims to be the current answer, so it cannot be stale; the current answer
  is still computed. This is the same category as `rule_dismissal` (a judgement) and `deletion` (a
  record of what was removed) — facts about what happened, which no amount of recomputation
  recovers.
- **Sightings are recorded by a review, not by ingestion.** Recording on ingest means running twelve
  rules on every arriving call, on the live path — the cost profile Phase 11 exists to have removed,
  and the reason task 6.12 already puts *only* high-severity rules there (`app.rs:191`). So
  `first_seen` means **first seen by a review**, which is honest: the tool noticed when it looked.
  A store nobody has opened the risk tab on has no sightings, and the page says so rather than
  implying nothing was found.

## Tasks

### The sighting ledger

- [x] **12.1** Migration 006: `rule_sighting (rule_id TEXT, tool_use_id TEXT, first_seen INTEGER,
  PRIMARY KEY (rule_id, tool_use_id))`. No foreign key to `tool_call`: the row must outlive the call
  it names, the same way `deletion` outlives what it describes. **Never purged** — `retention.rs`
  gets no `DELETE` for it, and the migration says why in the same voice as migration 005.
- [x] **12.2** `rules::record_sightings(conn, &[Finding], now) -> usize`, one `INSERT … ON CONFLICT
  DO NOTHING` per matched call. It needs the matched `tool_use_id`s, which the Phase 11 rollup does
  not return — so `rollup` gains a mode that yields ids, or `record_sightings` re-runs each live
  rule's compiled `WHERE` as an `INSERT … SELECT`. **Prefer the second:** it is one statement per
  rule with no ids crossing into Rust, and it is the same compiled fragment the count came from.
- [x] **12.3** The review records sightings as a side effect, which makes it a **mutating** command.
  Two consequences to handle rather than discover: the write goes through the single writer
  ([ADR-0007]) via `capture.writer().submit_blocking`, the way `dismiss` already does; and the write
  bumps `data_version`, which would invalidate the memo it just filled. Take the watermark **after**
  the sighting write, not before — the opposite of task 11.3's rule, and for the opposite reason:
  here the write is ours and is already accounted for in the answer.
- [x] **12.4** `Finding.first_seen: Option<i64>` — the earliest sighting for that rule, and `None`
  for a rule no review has ever recorded. Plus `Finding.new_calls: i64`, matched calls whose sighting
  was first written by *this* run. Both come from the same ledger read, one query for all rules.
- [x] **12.5** The risk view says it: "first seen 4 days ago" under each finding, and **"3 findings
  are new since the last review"** in the summary. A rule whose `first_seen` is older than its
  `first_at` is normal (the rule was added after the calls); a finding with no `first_seen` at all
  means this is the first review, and the page says that instead of "0 new".
- [x] **12.6** **ADR-0012 — store sightings, not findings.** The decision is *where the line is*:
  a derivation is recomputed, an observation is recorded. It names the rejected alternative — a
  `finding` table with a re-analyse button — and why the button does not fix it: ingestion, not rule
  changes, is what makes such a table stale, and re-analysing on every ingest is the work Phase 11
  removed. It also records what a sighting costs: a row per (rule, flagged call), which on the
  owner's store is a few hundred rows and on a store where a broad rule matches everything is one
  per call.

### Risk as a filter

- [x] **12.7** `TimelineFilter` gains `risk: Option<Severity>` and `rule_id: Option<String>`. These
  are the first fields that cannot be compiled from the store alone — a rule lives in a file — so
  `query::selection` takes the rule set as a second argument. Threaded explicitly through the five
  call sites (`query.rs:233, :300, :413, :455, :555`) and `cli::export`, never smuggled in through a
  global: which rules were in force is exactly the thing that must be visible at the call.
- [x] **12.8** The compiled fragment is the one Phase 11 already builds: every live rule of a
  severity `OR`-ed together (`rules::reconcile`), injected as `tc.tool_use_id IN (SELECT … WHERE
  <that>)`. **Dismissed rules do not count**, matching the posture table — a rule set aside stops
  counting against the project that tripped it, and it should stop filling the timeline too.
- [x] **12.9** `@risk:high` and `@rule:<id>` in the query bar, with autocomplete: the four
  severities, and rule ids from the review. `format`/`parse` round-trip them like every other key,
  and the hash gains `risk=` and `rule=` — no existing key changes, so a v1.1 link still restores.
- [x] **12.10** **State what a risk link means.** Task 5.6 made time absolute so a shared view keeps
  its meaning; `@risk:high` cannot be made absolute, because the rules are a file that changes. It
  is evaluated against the rules **in force when the link is opened**, and that is a deliberate
  exception, not an oversight — the alternative is embedding a rule definition in a URL. Said in
  `query.ts` beside the key, and in the ADR.
- [x] **12.11** The histogram comes free and must be checked rather than assumed: it is built on the
  same `selection()`, so `@risk:high` with no time bounds is **risk over time** — the chart Phase 10
  built, answering a question nobody has been able to ask. Measure it against the 200 ms budget,
  because the risk fragment is twelve `LIKE`/`GLOB` patterns and this is the first time it lands
  inside a per-bucket `GROUP BY`.
- [x] **12.12** A finding gains **"Open in the timeline"**, setting `@rule:<id>`. The inline first
  page stays; this is a route, not a replacement. And correct `rules.rs:718` — "a rule's conditions
  have no equivalent in `TimelineFilter`" stops being true in this phase, and a comment that
  explains a design by a fact that has changed is worse than no comment.
- [x] **12.13** Export inherits it, since Export takes the filter. `toolog export --risk high` on
  the CLI, and the same `--rule <id>`. This is the first time the CLI's export needs the rules file,
  so a missing or unparsable `rules.toml` has to fail loudly there rather than silently exporting
  everything.

## Exit criteria

- Opening the risk tab on a store that has never been reviewed records a sighting per flagged call,
  and says "first review" rather than "0 new".
- Re-opening it immediately after records **nothing** and still reports the same `first_seen` dates —
  the ledger is append-only and a second look is not a new sighting.
- `toolog purge --session <id> --apply` removes the calls and leaves the sightings, and the risk view
  says a finding's calls are gone rather than showing an empty drill-through.
- `@risk:high` in the timeline shows exactly the calls the risk view counts at `high`, asserted
  against `reconcile`'s own number over the Phase 11 `awkward()` fixture. A dismissed rule's calls
  are in neither.
- `parse(format(f)) === f` still holds for every field, including the two new ones, and a v1.1 hash
  link still restores.
- The histogram over `@risk:high` with no time bounds paints inside 200 ms on the owner's store, and
  the number is recorded here.
- `just check` green, with no `#[allow(dead_code)]` added to get there.

## Outcome

**Done.** `just check` is green: 160 frontend tests and the Rust suite. 1,079 lines added, 79
removed.

### Measured, on the owner's real store

4,404 calls, 12 rules. A first review recorded **193 sightings** — the exact sum of the nine
matching rules' call counts — and a second review recorded **0**, which is the append-only property
holding on real data rather than in a fixture.

| Histogram, no time bounds | Time |
|---|---|
| The whole store | 4.6 ms |
| `@risk:high` | **53.7 ms** |

Inside the 200 ms budget, and worth stating why it is twelve times the plain one: the risk subquery
is a full scan carrying a dozen `LIKE`/`GLOB` patterns, and it runs **twice** — once in
`called_at_span` to find the range, once in the bucket `GROUP BY`. Both go through `selection()`,
which is what keeps the chart and the list describing the same rows; collapsing them into one pass
would mean a second code path and is not worth 25 ms.

### Decisions the tasks left open

- **`Lens`, not a fifth parameter on every read.** `risk` and `rule_id` are the first
  `TimelineFilter` fields that cannot be compiled from the store alone. Threading `&[Rule]` through
  every signature would have touched ~40 test call sites that do not care; a `Lens` with
  `From<&TimelineFilter>` leaves them untouched. The safety comes from elsewhere: a `Lens::plain`
  filter that names a risk field is an **error**, not an empty result, and
  `a_filter_that_asks_for_risk_without_rules_is_an_error_not_an_empty_list` says so. Answering "show
  me the high-risk calls" with silence because nobody passed the rules is the wrong answer this
  crate exists not to give.
- **One `timeline_lens` helper in the app, not a lens per command.** All five timeline reads go
  through it, so no command can forget the rules. It skips reading the rules file entirely when the
  filter asks for nothing rule-shaped, which is the common case.
- **A failed sighting write does not fail the review.** A store that cannot be written to is still
  worth reviewing; the findings are right and only the ledger falls behind. It logs and re-reads
  rather than propagating.
- **`@risk` writes a plain string field but validates like a word control.** `high|medium|low|info`
  is checked in `parse`, so a typo is an inline error naming the four rather than a filter that
  silently matches nothing.
- **The histogram test needed its own benign call.** `awkward()` is destructive by design — every
  call in it trips something — so "not every call is high risk" had to be arranged in the test
  rather than by perturbing a fixture five other tests assert exact numbers against.

### The exit criteria

- **A first review records, a second does not.** Both asserted (`looking_twice_is_not_seeing_twice`)
  and measured on the real store: 193, then 0.
- **A purge leaves the sightings.** `a_sighting_outlives_the_call_it_names` purges a session and
  asserts the ledger is unchanged — the same property the `deletion` table has.
- **`@risk:high` shows exactly what the review counts at `high`**, asserted across all four
  severities against `reconcile`'s own numbers over the Phase 11 `awkward()` fixture. A dismissed
  rule's calls are in neither.
- **`parse(format(f)) === f` still holds**, including the two new keys — and the Phase 10 test that
  every key round-trips something is what caught them being added without cases.
- **A v1.1 hash link still restores.** No key changed; `risk=` and `rule=` were added.
- **`just check` green, no `#[allow(dead_code)]`.**

### Not verified

**Nobody has looked at the window.** Every assertion is a headless DOM or a Rust test. Specifically
unseen: whether "First review of this store" reads as informative or as an error, whether the
"first seen" row is confusable with "first at" sitting two rows above it, and whether
`@risk:high` in the query bar is discoverable at all without something on the risk view pointing at
it.

**The dead-end drill-through is designed and not built.** ADR-0012 says a sighting can name a call
that no longer exists and "the view has to say so rather than showing an empty list". Nothing in this
phase surfaces sightings for calls that are gone — `Finding.first_seen` comes from the ledger, but
the finding's counts come from the live store, so a purged call simply stops being counted. The
record is kept and is currently only reachable with `sqlite3`.

**`first_seen` is only as good as how often the tab is opened.** On a store reviewed once a month,
every finding dates to the day of the review. The number is honest about what it measures — when the
tool noticed — but it is not when the thing happened, and the view does not currently distinguish
them beyond showing both.
