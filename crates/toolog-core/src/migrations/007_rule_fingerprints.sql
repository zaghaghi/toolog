-- Migration 007 — a sighting records which *version* of a rule saw the call.
--
-- Migration 006 keyed sightings on `rule_id` alone, and a rule's id is chosen
-- by whoever writes the rules file. Keep the id and change what the rule looks
-- for and its sightings carried over, so `first_seen` reported a date that
-- described conditions no longer in force — measured: a rule retuned from
-- `rm -rf` to `dd if=` matched nothing and still reported the original date.
--
-- ADR-0012 argued a sighting cannot go stale because "the old sightings remain
-- true statements about what the old rule saw". True of the rows, and not of
-- the number the finding showed. The fingerprint makes it true of both.
--
-- The fingerprint covers the rule's `scope` and `match` — what it looks for.
-- Not its title, explanation or severity: renaming a rule is not asking a
-- different question, and should not throw away its history.
--
-- Existing rows are copied with the empty string, which means "recorded before
-- versions were tracked". `rules::adopt_sightings` claims them for the current
-- fingerprint on the next review — a one-time assumption that the rules have
-- not changed since they were written, which cannot be checked and is true for
-- anyone who has not edited `rules.toml`. Left unclaimed they would be dead
-- rows and every date would reset to today, which is worse.
CREATE TABLE rule_sighting_new (
    rule_id     TEXT    NOT NULL,
    -- Which version of the rule saw it. '' for rows written by migration 006.
    fingerprint TEXT    NOT NULL,
    tool_use_id TEXT    NOT NULL,
    first_seen  INTEGER NOT NULL,
    PRIMARY KEY (rule_id, fingerprint, tool_use_id)
) WITHOUT ROWID;

INSERT INTO rule_sighting_new (rule_id, fingerprint, tool_use_id, first_seen)
SELECT rule_id, '', tool_use_id, first_seen FROM rule_sighting;

DROP TABLE rule_sighting;
ALTER TABLE rule_sighting_new RENAME TO rule_sighting;

CREATE INDEX rule_sighting_seen ON rule_sighting (rule_id, fingerprint, first_seen);
