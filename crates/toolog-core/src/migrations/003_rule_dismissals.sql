-- Migration 003 — dismissed risk rules.
--
-- Findings are computed from the rules in force, never stored: change a rule
-- and the findings change with it (ADR-0004). A dismissal is the exception,
-- because it is a judgement a person made rather than something derived, and
-- re-running the rules must not throw it away.
--
-- It records a decision *about a rule*. It never touches, hides or deletes the
-- calls behind it — those stay in the timeline and in every export.
CREATE TABLE rule_dismissal (
    rule_id      TEXT    PRIMARY KEY,
    note         TEXT    NOT NULL,
    dismissed_at INTEGER NOT NULL
);
