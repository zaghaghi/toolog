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

- [ ] **11.1** One query per rule instead of four. `evaluate` runs a count aggregate
  (`rules.rs:437`), then `projects_for` (`:481`), then `examples_for` (`:496`), and `by_project`
  then runs a fourth (`:594-607`). A single `GROUP BY s.project_path` per rule yields the counts, the
  project list and the per-project posture together: the three are the same scan, sliced
  differently — which is also why the summary and the list still cannot disagree.
- [ ] **11.2** Drop `Finding.examples` and fetch on expand. Eight example rows are built for all
  twelve rules on every tab activation and read for at most one. The frontend already has the path:
  `moreCalls()` (`ui/src/risk.ts:229-259`) pages `rule_calls`, served by `rules::calls`
  (`rules.rs:520`). The first page becomes the first eight, rather than a second source of truth for
  them.
- [ ] **11.3** Memoize the review in `AppState`, guarded by three things read in well under a
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
- [ ] **11.4** Give the evaluation its own read connection so a slow review cannot block the
  timeline behind the shared mutex. `crates/toolog-app/src/state.rs:69-74` says why it is a mutex
  and not a pool — "every query under 3 ms on the real corpus … revisit when" — and this is the
  *when*. WAL already permits concurrent readers, so this is a second `Connection`, not a pool.
- [ ] **11.5** **ADR-0011 — memoize the risk review, do not materialize it.** ADR-0004's "computed,
  never stored" is about the *store*: a projection table can go stale in a database that outlives
  the process, and a memo a watermark invalidates cannot. The ADR names the rejected alternative — a
  `finding` table maintained on ingest — and why it would recreate exactly the reconciliation
  problem ADR-0004 exists to avoid.
- [ ] **11.6** Measure it on the owner's real store, before and after, in this document — the way
  Phases 6 and 7 recorded theirs.

### Adds up

- [ ] **11.7** One unit for the hero and the table: **distinct calls flagged**, `count(DISTINCT
  tc.tool_use_id)` per severity, so a call two rules caught is one call. The table's columns then
  sum to the hero exactly.
- [ ] **11.8** An explicit "no project recorded" row, because those calls are dropped from the table
  today (`rules.rs:598`) and counted in the hero. A test asserts the columns sum to the hero over a
  store built to hold every awkward case: a rule spanning three projects, a session-scoped rule, a
  call two rules catch, and a call whose session has no `project_path`.
- [ ] **11.9** State the one thing that still will not sum, rather than let a reader discover it.
  Each severity column reconciles with its own hero number, but the **four hero numbers do not add to
  a grand total** — a call caught by a `high` rule and a `low` rule is one call at each severity — so
  there is no grand total on the page. The other two scopes do reconcile, and it is worth recording
  why: a `Session` rule is already narrowed to the session's first call (`rules.rs:376-388`), so it
  contributes exactly one `tool_use_id` per session, and `RetryAfterRefusal` is a correlated subquery
  over `tool_call` rows (`rules.rs:390-422`), so its matches are ordinary calls.
- [ ] **11.10** Rule counts keep their place, as the secondary line under each hero number — "3
  rules, 184 calls". Both numbers are worth having; only one of them can be the total.

### Can be read

- [ ] **11.11** Every rule appears, including the ones that matched nothing. `evaluate` skips
  zero-match rules today (`rules.rs:466-468`), which is exactly why a reader cannot tell a rule
  exists. A rule that found nothing is a real result, the same way Phase 6's empty state is.
- [ ] **11.12** A rules panel on the risk view: id, title, severity, scope, built-in or replaced by
  the user file, the current match count, and **what it looks for in plain language** — rendered from
  the `Match` struct (`rules.rs:79-128`), not from a hand-written description that can drift from the
  rule it claims to describe. `first_line` and `outside_cwd` in particular need saying in words.
- [ ] **11.13** A `reveal_rules` command beside the existing `reveal_logs`
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

## Not verified

*To be filled in.*

## Outcome

*Not started.*
