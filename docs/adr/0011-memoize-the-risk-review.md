# ADR-0011 — Memoize the risk review; do not materialize it

- **Status:** Accepted
- **Date:** 2026-09-06
- **Relates to:** [ADR-0004](0004-store-raw-project-normalized.md),
  [ADR-0007](0007-single-resident-process.md), [ADR-0009](0009-correlate-on-tool-use-id.md)

## Context

The owner's report on v1.0 opened with the risk view:

> "It's very slow, it takes time to load the page; it seems like we always do the risk calculation."

We always did. `risk_review` ran on **every tab activation**, and each activation ran twelve rules
through four near-full scans of `tool_call` each — a count aggregate, a project list, eight example
rows, and a fourth pass in `by_project` — on the one read connection every other query shares.
Measured on the owner's store (4,221 calls, 151 MB): **2,314 ms**, of which one rule,
`retry-after-refusal`, was 2,125 ms on its own.

Phase 11 took most of that out by fixing the work itself — one query per rule instead of four, the
refusals gathered once instead of re-found per candidate row, examples fetched only when a finding is
expanded. That got it to **97 ms**. But 97 ms is still 97 ms *every time the tab is touched*, for an
answer that has not changed since the last time, on a store where minutes pass between captures.

The obvious next step is to store the findings. That is the step this ADR refuses.

**ADR-0004 already says findings are computed, never stored** — and `rules.rs` says why: change a
rule and the findings change with it, with no stale rows to reconcile. It is worth being precise
about what that claim covers, because "do not store it" and "do not compute it twice" are different
claims and only the first one is ADR-0004's.

ADR-0004 is about **the store**: a database that outlives the process, is written by one process and
read by others, and can be modified by `toolog purge` or a re-projection while nothing is watching. A
`finding` table in it can go stale in ways nothing detects. An in-memory memo in a single resident
process ([ADR-0007]) is a different object: it lives and dies with the process, and it can be tied to
a watermark that moves whenever the answer could have changed.

[ADR-0007]: 0007-single-resident-process.md

## Decision

**Cache the computed review in memory, guarded by three facts that are cheaper to check than the
answer is to recompute. Never write findings to the database.**

The three guards, and why each is needed:

1. **`PRAGMA data_version` on the risk connection.** It moves on any commit by *another* connection,
   which is exactly the set of events that can change a review.
2. **The user rules file's mtime.** Editing `rules.toml` changes the answer without touching the
   store at all. `None` — no file — is itself a state, so writing one for the first time retires the
   memo.
3. **A dismissal counter**, bumped by the commands that record a judgement. `data_version` catches
   these too, since a dismissal is a commit by the writer; the counter is kept because it is exact
   and free, and because the command that writes the judgement is the right place to invalidate.

**The evaluation gets its own read connection** ([Phase 11](../phases/11-risk-fast-and-legible.md),
task 11.4). Two reasons, and the second is not optional: a slow review must not hold the timeline
behind the shared mutex, *and* `PRAGMA data_version` reports commits by other connections — read on
the connection doing the writing it would never move, and the memo would never expire. WAL already
permits concurrent readers, so this is a second `Connection`, not a pool.

The alternatives to `data_version` were measured against the lanes rather than assumed:

- **`max(rowid)` on `tool_call`** misses the OTEL lane *updating* a row the transcript created —
  which is the arrival that adds the `decision` most of these rules read ([ADR-0009]).
- **The writer's update hook** (`writer.rs`) ignores `SQLITE_DELETE` and is only installed when a
  live sink exists, so `toolog purge` and a headless run both slip past it.
- **`data_version`** covers all of those and costs one pragma read rather than a `count(*)`.

Verified on a real store rather than trusted: it moves on an insert, on an update of an existing row,
and on a delete by another connection — and does **not** move for the writing connection's own
write, which is the property that makes the separate connection necessary rather than tidy.

## Consequences

**Positive**

- Re-opening the risk tab with nothing newly captured issues one pragma read and returns the same
  answer. That is the owner's complaint, closed.
- The invariant ADR-0004 protects is untouched: nothing derived is on disk, so nothing derived can
  be stale on disk. Delete the process and the memo is gone with it.
- The guards are falsifiable. A memo that survives a change it should not is a test, not a mystery.

**Negative**

- The first review after any capture still costs 97 ms. Memoization does not make a cold answer fast;
  it makes a repeated one free. A store an order of magnitude larger would need the work reduced
  again, not cached harder.
- A memo is process state, so two windows of the same process share it and a restart discards it.
  Both are correct here and would not be if the memo were a claim about the world rather than about
  one process's last answer.

**Neutral**

- The memo holds a full `RiskReview`, which is the findings for twelve rules — kilobytes, not
  megabytes, now that eight example calls per rule no longer ride along with it.

## Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| A `finding` table maintained on ingest | Recreates exactly the reconciliation problem ADR-0004 exists to avoid: a derived table in a database that outlives the process, which a rules-file edit, a `toolog purge` or a re-projection can silently invalidate. It would also make "rules are data you can edit" false — editing a rule would leave rows describing the old one until something noticed. |
| Recompute on a timer, serve the last answer meanwhile | Trades a slow correct answer for a fast possibly-wrong one, and adds a background job to a process whose whole design is one resident pipeline. The watermark is cheaper than the timer and cannot be out of date. |
| Invalidate only on the app's own writes | Misses every writer that is not this window: `toolog backfill` in a terminal, `toolog purge`, another instance. The store is a file other processes touch, and a cache that assumes otherwise is wrong exactly when it matters. |
| Leave it uncached and keep optimizing the queries | 97 ms is respectable and the queries are now the shape they should be, but it is still work redone for an unchanged answer on every tab click. The cheapest query is the one not issued. |
