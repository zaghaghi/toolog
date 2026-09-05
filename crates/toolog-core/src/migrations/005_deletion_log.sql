-- Migration 005 — the record of what was removed (tasks 7.4 and 7.8).
--
-- Deleting evidence breaks the integrity chain, and it should: the chain's job
-- is to notice that stored records changed, and they did. What it must not do
-- is leave the break unexplained, so every purge writes a row here first —
-- what was removed, why, when, and the chain values either side of the hole.
-- `toolog verify --chain` reads this and reports a break as accounted for or
-- not, which is the difference between routine retention and tampering.
--
-- This table is never purged. It is small, it is the only thing that survives
-- what it describes, and a retention policy that eventually erased the record
-- of its own deletions would be self-defeating.
CREATE TABLE deletion (
    id          INTEGER PRIMARY KEY,
    at          INTEGER NOT NULL,
    -- 'retention' (an age or size cap), 'session', or 'project'.
    reason      TEXT    NOT NULL,
    -- The cutoff, session id or project path this was scoped to, in words.
    detail      TEXT    NOT NULL,
    raw_events  INTEGER NOT NULL,
    tool_calls  INTEGER NOT NULL,
    sessions    INTEGER NOT NULL,
    -- The span of `raw_event` ids removed, so a break can be matched to it.
    first_id    INTEGER,
    last_id     INTEGER,
    -- The chain value of the last surviving record before the hole. A break
    -- reported at a row whose predecessor's chain matches this is explained.
    chain_before TEXT
);

CREATE INDEX deletion_at ON deletion (at DESC);
