-- Migration 006 — when a finding was first seen (task 12.1).
--
-- Findings are still not stored. A finding is a *derivation*: recompute it and
-- you get today's answer from today's rules, which is what ADR-0004 and
-- ADR-0011 are about. A sighting is an *observation* — this rule caught this
-- call, and we first noticed on this date — and no amount of recomputation
-- recovers a date. That is the line, and this table sits on the same side of it
-- as `rule_dismissal` (a judgement someone made) and `deletion` (a record of
-- what was removed).
--
-- Append-only, and it never claims to be the current answer: a row here says
-- "this was flagged, then", not "this is flagged". So it cannot go stale the
-- way a `finding` table would — retune a rule and the old sightings are still
-- true statements about what the old rule saw.
--
-- **No foreign key to `tool_call`, and never purged.** The row has to outlive
-- the call it names, exactly as `deletion` outlives what it describes: "this
-- was flagged before you deleted it" is a thing an audit trail should still be
-- able to say. `retention.rs` gets no DELETE for this table.
CREATE TABLE rule_sighting (
    rule_id     TEXT    NOT NULL,
    tool_use_id TEXT    NOT NULL,
    -- When a review first recorded it, not when the call ran. `tool_call` holds
    -- the second; this is the one nothing else can reconstruct.
    first_seen  INTEGER NOT NULL,
    PRIMARY KEY (rule_id, tool_use_id)
) WITHOUT ROWID;

-- "What is new since Tuesday" and "when did this rule start firing" are both
-- reads over one rule ordered by time.
CREATE INDEX rule_sighting_seen ON rule_sighting (rule_id, first_seen);
