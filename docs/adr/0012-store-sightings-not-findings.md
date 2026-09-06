# ADR-0012 — Store sightings, not findings

- **Status:** Accepted
- **Date:** 2026-09-06
- **Relates to:** [ADR-0004](0004-store-raw-project-normalized.md),
  [ADR-0007](0007-single-resident-process.md), [ADR-0011](0011-memoize-the-risk-review.md)

## Context

The risk review can say *what* is wrong and not *when we first noticed*. `Finding.first_at` and
`last_at` are `min`/`max` of `tool_call.called_at` — when the **call** ran. Add a rule today and a
three-month-old call reads as three months old; nothing on the screen distinguishes *old news* from
*new to me*. A purge makes it worse: `retention.rs` deletes the calls, and the finding goes with
them, because the finding was only ever computed from them. The record that something was flagged
does not survive the thing it described.

The obvious fix is a `finding` table with a "re-analyse" button after a rule change. ADR-0004 and
ADR-0011 have both refused to store findings, and it is worth saying precisely why the button does
not rescue the idea:

- **Ingestion, not rule changes, is what makes such a table stale.** Every captured call can create
  findings. A `finding` table is wrong seconds after any capture unless it is maintained on ingest —
  which means running twelve rules on the live path, the cost profile [ADR-0011](0011-memoize-the-risk-review.md)
  exists to have removed, and the reason task 6.12 puts only *high-severity* rules there.
- **A stored finding outlives the rule it names.** "Rule X matched call Y" becomes a claim about a
  rule that no longer exists the moment X is retuned. Keeping it honest means versioning rules or
  storing the matched conditions beside each row — real complexity, for a row whose current truth is
  95 ms away.

But the *timing* question is not answered by recomputation at all. No amount of re-running the rules
recovers when something was first seen. That is not a derivation. It is history.

## Decision

**Draw the line between a derivation and an observation. Recompute the first; record the second.**

`rule_sighting (rule_id, tool_use_id, first_seen)` — append-only, primary key on the pair, no foreign
key to `tool_call`, and never purged.

1. **A sighting is not a finding.** A row says "this was flagged, then", never "this is flagged". So
   it cannot go stale the way a `finding` table would: retune a rule and the old sightings remain
   true statements about what the old rule saw. The current answer is still computed.
2. **Written by a review, not by ingestion.** `first_seen` means *first seen by a review* — the tool
   noticed when it looked. A store nobody has opened the risk tab on has no sightings, and the page
   says "first review" rather than "0 new", because those are different statements and the second
   one is reassurance it has not earned.
3. **Written from the same compiled fragment the count came from.** `record_sightings` runs each live
   rule's `where_for` as an `INSERT … SELECT`, so a sighting cannot exist for a call the finding did
   not report. `ON CONFLICT DO NOTHING` makes looking twice not seeing twice, and the affected-row
   count is therefore exactly what this run saw for the first time.
4. **Dismissed rules record nothing.** A rule set aside is not being watched, and filling "new since
   the last review" with it would report things nobody asked to be told about.
5. **It outlives what it describes**, exactly as the `deletion` table does. "This was flagged before
   you deleted it" is a thing an audit trail should still be able to say.

This puts `rule_sighting` on the same side of the line as the two tables already there:
`rule_dismissal` is a judgement a person made, `deletion` is a record of what was removed. None of
the three is recoverable by recomputation, which is what makes storing them different from storing
findings.

**One consequence, stated rather than discovered:** a review now writes. It goes through the single
writer ([ADR-0007]) like a dismissal does, and that write moves `data_version` — so the memo's
watermark is taken **after** the sighting write, inverting [ADR-0011](0011-memoize-the-risk-review.md)'s
rule for the opposite reason. There the write was someone else's and had to expire the memo; here it
is ours and is already accounted for in the answer being cached.

[ADR-0007]: 0007-single-resident-process.md

## Consequences

**Positive**

- "When did this start?", "what is new since I last looked?" and "was this flagged before I deleted
  it?" all become answerable, none of them by storing a derivation.
- The ledger is small: one row per (rule, flagged call). On the owner's store, a first review
  recorded **193** rows.
- It cannot drift from the review, because it is written by the review from the review's own
  compiled conditions rather than from a second query that could ask a different question.

**Negative**

- A sighting can name a call that no longer exists, and the drill-through into it is a dead end. That
  is the cost of outliving what it describes, and the view has to say so rather than showing an empty
  list.
- `first_seen` is only as good as how often the tab is opened. A store reviewed once a month dates
  every finding to the day of the review, not the day of the call. The number is honest about what it
  measures — when the tool noticed — but it is not when the thing happened, and the two are easy to
  confuse.
- A broad rule matching most of the store makes the ledger one row per call. Bounded by
  rules × flagged calls, which is bounded by the store, but it is not free.

**Neutral**

- The review is now a mutating command. Nothing else about its shape changes, and a store that cannot
  be written to still reviews — the findings are right and only the ledger falls behind.

## Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| A `finding` table, re-analysed on demand | Ingestion is what makes it stale, not rule changes, so the button does not fix it — and maintaining it on ingest is the work ADR-0011 removed. A stored row also outlives the rule it names. |
| Record sightings on ingest, so notifications fire live | Twelve rules on every arriving call, on the live path. Task 6.12 already established that only high-severity rules are affordable there, and it opens its own connection per call to do it. The trade is real: "new findings" now appear when you look rather than when they happen. |
| Store `last_seen` as well | It is `max(called_at)` of the current matches, which is computed and already on the finding. Storing it would be storing a derivation next to an observation and inviting the two to disagree. |
| Purge sightings with their calls | Makes the ledger unable to answer the one question a purge creates: what was flagged before it was deleted. The `deletion` table already establishes that a record of what was removed must outlive the removal. |
