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

- [ ] **12.1** Migration 006: `rule_sighting (rule_id TEXT, tool_use_id TEXT, first_seen INTEGER,
  PRIMARY KEY (rule_id, tool_use_id))`. No foreign key to `tool_call`: the row must outlive the call
  it names, the same way `deletion` outlives what it describes. **Never purged** — `retention.rs`
  gets no `DELETE` for it, and the migration says why in the same voice as migration 005.
- [ ] **12.2** `rules::record_sightings(conn, &[Finding], now) -> usize`, one `INSERT … ON CONFLICT
  DO NOTHING` per matched call. It needs the matched `tool_use_id`s, which the Phase 11 rollup does
  not return — so `rollup` gains a mode that yields ids, or `record_sightings` re-runs each live
  rule's compiled `WHERE` as an `INSERT … SELECT`. **Prefer the second:** it is one statement per
  rule with no ids crossing into Rust, and it is the same compiled fragment the count came from.
- [ ] **12.3** The review records sightings as a side effect, which makes it a **mutating** command.
  Two consequences to handle rather than discover: the write goes through the single writer
  ([ADR-0007]) via `capture.writer().submit_blocking`, the way `dismiss` already does; and the write
  bumps `data_version`, which would invalidate the memo it just filled. Take the watermark **after**
  the sighting write, not before — the opposite of task 11.3's rule, and for the opposite reason:
  here the write is ours and is already accounted for in the answer.
- [ ] **12.4** `Finding.first_seen: Option<i64>` — the earliest sighting for that rule, and `None`
  for a rule no review has ever recorded. Plus `Finding.new_calls: i64`, matched calls whose sighting
  was first written by *this* run. Both come from the same ledger read, one query for all rules.
- [ ] **12.5** The risk view says it: "first seen 4 days ago" under each finding, and **"3 findings
  are new since the last review"** in the summary. A rule whose `first_seen` is older than its
  `first_at` is normal (the rule was added after the calls); a finding with no `first_seen` at all
  means this is the first review, and the page says that instead of "0 new".
- [ ] **12.6** **ADR-0012 — store sightings, not findings.** The decision is *where the line is*:
  a derivation is recomputed, an observation is recorded. It names the rejected alternative — a
  `finding` table with a re-analyse button — and why the button does not fix it: ingestion, not rule
  changes, is what makes such a table stale, and re-analysing on every ingest is the work Phase 11
  removed. It also records what a sighting costs: a row per (rule, flagged call), which on the
  owner's store is a few hundred rows and on a store where a broad rule matches everything is one
  per call.

### Risk as a filter

- [ ] **12.7** `TimelineFilter` gains `risk: Option<Severity>` and `rule_id: Option<String>`. These
  are the first fields that cannot be compiled from the store alone — a rule lives in a file — so
  `query::selection` takes the rule set as a second argument. Threaded explicitly through the five
  call sites (`query.rs:233, :300, :413, :455, :555`) and `cli::export`, never smuggled in through a
  global: which rules were in force is exactly the thing that must be visible at the call.
- [ ] **12.8** The compiled fragment is the one Phase 11 already builds: every live rule of a
  severity `OR`-ed together (`rules::reconcile`), injected as `tc.tool_use_id IN (SELECT … WHERE
  <that>)`. **Dismissed rules do not count**, matching the posture table — a rule set aside stops
  counting against the project that tripped it, and it should stop filling the timeline too.
- [ ] **12.9** `@risk:high` and `@rule:<id>` in the query bar, with autocomplete: the four
  severities, and rule ids from the review. `format`/`parse` round-trip them like every other key,
  and the hash gains `risk=` and `rule=` — no existing key changes, so a v1.1 link still restores.
- [ ] **12.10** **State what a risk link means.** Task 5.6 made time absolute so a shared view keeps
  its meaning; `@risk:high` cannot be made absolute, because the rules are a file that changes. It
  is evaluated against the rules **in force when the link is opened**, and that is a deliberate
  exception, not an oversight — the alternative is embedding a rule definition in a URL. Said in
  `query.ts` beside the key, and in the ADR.
- [ ] **12.11** The histogram comes free and must be checked rather than assumed: it is built on the
  same `selection()`, so `@risk:high` with no time bounds is **risk over time** — the chart Phase 10
  built, answering a question nobody has been able to ask. Measure it against the 200 ms budget,
  because the risk fragment is twelve `LIKE`/`GLOB` patterns and this is the first time it lands
  inside a per-bucket `GROUP BY`.
- [ ] **12.12** A finding gains **"Open in the timeline"**, setting `@rule:<id>`. The inline first
  page stays; this is a route, not a replacement. And correct `rules.rs:718` — "a rule's conditions
  have no equivalent in `TimelineFilter`" stops being true in this phase, and a comment that
  explains a design by a fact that has changed is worse than no comment.
- [ ] **12.13** Export inherits it, since Export takes the filter. `toolog export --risk high` on
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

## Not verified

*To be filled in.*

## Outcome

*Not started.*
