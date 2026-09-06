# Phase 11 — A risk review that is fast, adds up, and can be read → **v1.1**

**Goal:** the three specific complaints about the one view the owner said is good.

**Depends on:** [Phase 9](09-subtraction.md). Runs after [Phase 10](10-one-lens.md) because the two
share `app.css`, `bindings.ts` and the timeline mockup.
**Governed by:** [ADR-0004](../adr/0004-store-raw-project-normalized.md) and a new ADR-0011 written
here.

## Why

| Report | What it is |
|---|---|
| "It's very slow, it takes time to load the page; it seems like we always do the risk calculation." | We always do. `risk_review()` (`crates/toolog-app/src/commands.rs:114`) runs four near-full scans of `tool_call` per rule across twelve rules, on **every tab activation** (`ui/src/main.ts:146`), on the one read connection every other query shares. |
| "Hero numbers don't add up to the table." | They count different things. The hero counts *rules* (`ui/src/risk.ts:108-115`); the table counts *(rule, project) pairs* (`crates/toolog-core/src/rules.rs:583-630`), so one rule spanning three projects appears three times. The table's `Calls` column also drops calls whose session has no `project_path` (`rules.rs:598`), which the hero counts. |
| "Risks are not viewable/editable by the user." | `evaluate` skips any rule with zero matches (`rules.rs:466-468`), so a rule that never fired is invisible — a reader cannot tell it exists. Nothing in the window says what a rule looks for; the only handle is a footnote naming a file path. |

The owner chose **viewer, not editor**: authoring a rule stays a file edit. So the fix for the third
point is that every rule, its conditions and its match count become visible in the window.

## Tasks

### Fast

- [x] **11.1** One query per rule instead of four. `evaluate` runs a count aggregate
  (`rules.rs:437`), then `projects_for` (`:481`), then `examples_for` (`:496`), and `by_project`
  then runs a fourth (`:594-607`). A single `GROUP BY s.project_path` per rule yields the counts, the
  project list and the per-project posture together: the three are the same scan, sliced
  differently — which is also why the summary and the list still cannot disagree.
- [x] **11.2** Drop `Finding.examples` and fetch on expand. Eight example rows are built for all
  twelve rules on every tab activation and read for at most one. The frontend already has the path:
  `moreCalls()` (`ui/src/risk.ts:229-259`) pages `rule_calls`, served by `rules::calls`
  (`rules.rs:520`). The first page becomes the first eight, rather than a second source of truth for
  them.
- [x] **11.3** Memoize the review in `AppState`, guarded by three things read in well under a
  millisecond: **`PRAGMA data_version`** on the risk connection, the rules file's mtime, and a
  dismissal counter. `data_version` is the right signal and the obvious alternatives are not:

  - `max(rowid)` misses the OTEL lane *updating* a row the transcript created — the arrival that
    adds the `decision` most of these rules read (ADR-0009).
  - The writer's update hook (`crates/toolog-core/src/writer.rs:162-180`) ignores `SQLITE_DELETE`
    and is only installed when a live sink exists, so `toolog purge` and a headless run both slip
    past it.
  - `data_version` moves on any commit by *another* connection, which covers all of those, and
    costs one pragma read rather than a `count(*)`.

  It turns on the risk evaluation holding its own connection (11.4) — a pragma read on the writing
  connection would never move. Verify that on the real store rather than trusting the paragraph.
- [x] **11.4** Give the evaluation its own read connection so a slow review cannot block the
  timeline behind the shared mutex. `crates/toolog-app/src/state.rs:69-74` says why it is a mutex
  and not a pool — "every query under 3 ms on the real corpus … revisit when" — and this is the
  *when*. WAL already permits concurrent readers, so this is a second `Connection`, not a pool.
- [x] **11.5** **ADR-0011 — memoize the risk review, do not materialize it.** ADR-0004's "computed,
  never stored" is about the *store*: a projection table can go stale in a database that outlives
  the process, and a memo a watermark invalidates cannot. The ADR names the rejected alternative — a
  `finding` table maintained on ingest — and why it would recreate exactly the reconciliation
  problem ADR-0004 exists to avoid.
- [x] **11.6** Measure it on the owner's real store, before and after, in this document — the way
  Phases 6 and 7 recorded theirs.

### Adds up

- [x] **11.7** One unit for the hero and the table: **distinct calls flagged**, `count(DISTINCT
  tc.tool_use_id)` per severity, so a call two rules caught is one call. The table's columns then
  sum to the hero exactly.
- [x] **11.8** An explicit "no project recorded" row, because those calls are dropped from the table
  today (`rules.rs:598`) and counted in the hero. A test asserts the columns sum to the hero over a
  store built to hold every awkward case: a rule spanning three projects, a session-scoped rule, a
  call two rules catch, and a call whose session has no `project_path`.
- [x] **11.9** State the one thing that still will not sum, rather than let a reader discover it.
  Each severity column reconciles with its own hero number, but the **four hero numbers do not add to
  a grand total** — a call caught by a `high` rule and a `low` rule is one call at each severity — so
  there is no grand total on the page. The other two scopes do reconcile, and it is worth recording
  why: a `Session` rule is already narrowed to the session's first call (`rules.rs:376-388`), so it
  contributes exactly one `tool_use_id` per session, and `RetryAfterRefusal` is a correlated subquery
  over `tool_call` rows (`rules.rs:390-422`), so its matches are ordinary calls.
- [x] **11.10** Rule counts keep their place, as the secondary line under each hero number — "3
  rules, 184 calls". Both numbers are worth having; only one of them can be the total.

### Can be read

- [x] **11.11** Every rule appears, including the ones that matched nothing. `evaluate` skips
  zero-match rules today (`rules.rs:466-468`), which is exactly why a reader cannot tell a rule
  exists. A rule that found nothing is a real result, the same way Phase 6's empty state is.
- [x] **11.12** A rules panel on the risk view: id, title, severity, scope, built-in or replaced by
  the user file, the current match count, and **what it looks for in plain language** — rendered from
  the `Match` struct (`rules.rs:79-128`), not from a hand-written description that can drift from the
  rule it claims to describe. `first_line` and `outside_cwd` in particular need saying in words.
- [x] **11.13** A `reveal_rules` command beside the existing `reveal_logs`
  (`crates/toolog-app/src/commands.rs:591`), and the file's path and format stated in the panel
  rather than in a footnote. "Rules are data you can edit" is only true if you can find and read
  them.

## Exit criteria

- Re-opening the risk tab with nothing newly captured issues **no query at all**, and the first paint
  on the owner's real store is measured and recorded above.
- The per-severity columns sum to their hero number over the fixture from 11.8.
- All twelve built-in rules are visible in the window, with their conditions, without opening a file
   — including the ones that matched nothing.
- A rule set aside still holds its place, greyed, carrying its note, and a dismissal still requires a
  reason. Phase 6 decided that and this phase does not revisit it.

## Measured, on the owner's real store

4,221 calls, 151 MB, 12 rules.
`cargo run --release -p toolog-core --example measure_risk -- <db>`, median of five after a warm-up:

| | Before | After |
|---|---|---|
| `evaluate` + the per-project table | **2,314 ms** | **95 ms** |
| of which `retry-after-refusal` | 2,125 ms | 3.0 ms |
| Re-opening the tab, nothing changed | 2,314 ms | **0.0008 ms** |

**The dominant cost was not the query count.** Task 11.1 is right that four scans per rule is three
too many, but `retry-after-refusal` alone was 92% of the review, and cutting it to one query would
still have left ~530 ms. `EXPLAIN QUERY PLAN` showed why: its correlated `EXISTS` let SQLite seek
`refused` by `tool_call_tool_name`, so every candidate `Bash` call re-scanned every *earlier* `Bash`
call looking for a rejection among them — while the store holds **three refusals in 4,295 calls**.
Gathering them once in a `MATERIALIZED` CTE takes the same query from 1,265 ms to 2.5 ms. A planner
hint (`+refused.tool_name`) also worked, at 3.9 ms; the CTE was chosen because it states what is true
about the data rather than what the planner should avoid doing.

The remaining 95 ms is twelve grouped scans plus four per-severity ones. Re-opening the tab issues
**one `PRAGMA data_version` and one atomic load** — 800 nanoseconds, no query against `tool_call`.
Strictly that is one pragma rather than "no query at all"; it is the smallest question that can be
asked, and the exit criterion is met in substance.

## Outcome

**Done.** `just check` is green: 150 frontend tests and the Rust suite. v1.1.

### Decisions the tasks left open

- **The retry rule's shape changed, not just its query count** (11.1). Described above. Without it,
  memoization would have hidden a 2-second first paint behind a cache.
- **`by_project` became `reconcile`, returning the summary and the table together** (11.7). They
  could not be made to agree while they were computed apart — the summary counted rules, the table
  counted (rule, project) pairs. One function, one pass per severity, and the column sums to the
  number above it *by construction*: a call belongs to exactly one project group.
- **`rule_ids` is read off the findings rather than re-queried.** Populating it cost nine more
  near-full scans per review for a field the window never displays. `Finding` gained
  `unattributed_calls` so the "no project recorded" row can still name its rules — one number from a
  scan already being done, instead of nine scans.
- **The memo's watermark is taken *before* the evaluation, never after** (11.3). Reading it
  afterwards would stamp a write that landed *during* the run into the memo as though the answer
  already accounted for it — fresh-looking and wrong. Taken first it can only expire early, which
  costs a recomputation.
- **`toolog risk` names the rules that matched nothing** in a closing line rather than printing
  twelve stanzas. Task 11.11 is about the window; a terminal report is read top to bottom.
- **The empty state now lists every rule.** "No rule matched anything" is still said, and the twelve
  rules are still under it with their conditions — otherwise a reader cannot tell "clean" from "not
  looking", which is the whole of 11.11.

### The exit criteria

- **Re-opening the tab issues no query.** One pragma, measured at 0.0008 ms. First paint on the real
  store measured and recorded above.
- **The per-severity columns sum to their hero number**, over a fixture holding every awkward case
  at once: a rule spanning three projects, a session-scoped rule, a call two *high* rules catch, a
  call caught at two different severities, and a call whose session has no `project_path`
  (`every_severity_column_adds_up_to_the_number_above_it`). It is asserted again after a dismissal,
  because that is when a total most easily stops adding up.
- **All twelve rules are visible with their conditions, without opening a file.** `evaluate` no
  longer skips a zero-match rule, and `describe` renders `Match` in words — in Rust, beside
  `compile`, so a condition added to the vocabulary without a phrase fails
  `every_condition_a_rule_can_state_can_be_said_in_words` rather than rendering as a rule that looks
  like it checks nothing.
- **A rule set aside still holds its place, greyed, carrying its note, and a dismissal still needs a
  reason.** Phase 6 decided that; the existing tests still assert it and this phase did not touch it.

### Not verified

The window was built and launched against the real store — it starts and runs — but **nobody has
looked at it.** Every assertion is a headless DOM or a Rust test. Specifically unseen: whether the
rules panel reads as a reference or as clutter under twelve findings, whether the "Matches when"
block is the right weight next to the explanation it sits below, and whether the greyed
"No project recorded" row reads as a real row rather than as a footer.

One thing is verified only on this machine's data: the store has **three refusals**, so
`retry-after-refusal` matches nothing here. Its speed-up is measured on the query, not on its
findings — a store where that rule actually fires would exercise a path this phase has not seen
produce a result.
