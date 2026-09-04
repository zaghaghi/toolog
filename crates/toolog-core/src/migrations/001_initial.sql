-- Migration 001 — initial schema.
--
-- The organising principle is ADR-0004: `raw_event` is the evidence, and every
-- other table is a derived, re-runnable projection of it. Claude Code's formats
-- drift (12 versions and 21 record types in one ordinary user's history), and
-- data lost at ingestion cannot be recovered.

-- ---------------------------------------------------------------------------
-- raw_event — append-only evidence. Written once, never updated.
-- ---------------------------------------------------------------------------
CREATE TABLE raw_event (
    id             INTEGER PRIMARY KEY,
    lane           TEXT    NOT NULL CHECK (lane IN ('transcript', 'otlp')),
    source_ref     TEXT    NOT NULL,
    source_offset  INTEGER,
    -- Makes re-ingestion idempotent, which is what lets the Phase 2 tailer
    -- recover from truncation by rescanning a file from zero.
    content_sha256 TEXT    NOT NULL UNIQUE,
    ingested_at    INTEGER NOT NULL,
    body           TEXT    NOT NULL
);

CREATE INDEX raw_event_source ON raw_event (source_ref, source_offset);
CREATE INDEX raw_event_lane   ON raw_event (lane, id);

-- ---------------------------------------------------------------------------
-- session
-- ---------------------------------------------------------------------------
CREATE TABLE session (
    session_id      TEXT PRIMARY KEY,
    project_path    TEXT,
    transcript_path TEXT,
    cwd             TEXT,
    git_branch      TEXT,
    cc_version      TEXT,
    entrypoint      TEXT,
    -- From `agent-name` records. 271 of 2,334 calls in the planning corpus are
    -- sidechain, so subagent attribution is not an edge case.
    agent_name      TEXT,
    first_seen      INTEGER,
    last_seen       INTEGER
);

CREATE INDEX session_last_seen ON session (last_seen DESC);
CREATE INDEX session_project   ON session (project_path);

-- ---------------------------------------------------------------------------
-- tool_call — one row per invocation, assembled from both lanes.
--
-- `session_id` deliberately carries no foreign key. The lanes race (ADR-0009):
-- an OTLP decision event can arrive before the transcript line that creates the
-- session, and a hard constraint would reject the very rejected-call rows this
-- tool exists to capture.
-- ---------------------------------------------------------------------------
CREATE TABLE tool_call (
    tool_use_id     TEXT PRIMARY KEY,
    session_id      TEXT,
    prompt_id       TEXT,
    message_uuid    TEXT,
    parent_uuid     TEXT,
    is_sidechain    INTEGER,
    agent_name      TEXT,
    tool_name       TEXT,
    tool_kind       TEXT,     -- builtin | mcp | skill | agent
    mcp_server      TEXT,
    mcp_tool        TEXT,
    called_at       INTEGER,
    completed_at    INTEGER,

    -- Transcript lane owns these. OTEL's truncated input must never overwrite
    -- them (ADR-0009).
    input_json      TEXT,
    input_summary   TEXT,
    target_path     TEXT,
    result_json     TEXT,
    result_text     TEXT,     -- plain text extracted for FTS
    result_size     INTEGER,
    success         INTEGER,

    -- OTLP lane owns these. Transcripts record none of them.
    duration_ms     INTEGER,
    error_type      TEXT,
    decision        TEXT,     -- accept | reject
    decision_source TEXT,     -- config | hook | user_permanent | user_temporary
                              -- | user_abort | user_reject
    permission_mode TEXT,

    -- Bit 1 transcript, bit 2 otlp. A row with only bit 2 is a rejected call;
    -- a row with only bit 1 is a gap in collection.
    provenance      INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX tool_call_called_at  ON tool_call (called_at DESC);
CREATE INDEX tool_call_session    ON tool_call (session_id, called_at DESC);
CREATE INDEX tool_call_tool_name  ON tool_call (tool_name, called_at DESC);
CREATE INDEX tool_call_prompt     ON tool_call (prompt_id);
CREATE INDEX tool_call_provenance ON tool_call (provenance);
CREATE INDEX tool_call_decision   ON tool_call (decision, decision_source);
CREATE INDEX tool_call_target     ON tool_call (target_path);

-- ---------------------------------------------------------------------------
-- file_change — from Edit/Write `structuredPatch`.
-- Ordering here is controllable, so the foreign key is safe.
-- ---------------------------------------------------------------------------
CREATE TABLE file_change (
    id            INTEGER PRIMARY KEY,
    tool_use_id   TEXT    NOT NULL REFERENCES tool_call (tool_use_id) ON DELETE CASCADE,
    file_path     TEXT    NOT NULL,
    lines_added   INTEGER NOT NULL DEFAULT 0,
    lines_removed INTEGER NOT NULL DEFAULT 0,
    patch_json    TEXT
);

CREATE INDEX file_change_tool_use ON file_change (tool_use_id);
CREATE INDEX file_change_path     ON file_change (file_path);

-- ---------------------------------------------------------------------------
-- api_request — OTLP only. Backfilled history has no cost data, and the UI must
-- say so rather than render a misleading zero.
-- ---------------------------------------------------------------------------
CREATE TABLE api_request (
    request_id            TEXT PRIMARY KEY,
    session_id            TEXT,
    prompt_id             TEXT,
    model                 TEXT,
    cost_usd_micros       INTEGER,
    input_tokens          INTEGER,
    output_tokens         INTEGER,
    cache_read_tokens     INTEGER,
    cache_creation_tokens INTEGER,
    duration_ms           INTEGER,
    speed                 TEXT,
    effort                TEXT,
    query_source          TEXT,
    agent_name            TEXT,
    ts                    INTEGER
);

CREATE INDEX api_request_session ON api_request (session_id, ts DESC);
CREATE INDEX api_request_ts      ON api_request (ts DESC);
CREATE INDEX api_request_model   ON api_request (model);

-- ---------------------------------------------------------------------------
-- prompt — length and command name only.
--
-- There is deliberately no column for prompt text. ADR-0008 never sets
-- OTEL_LOG_USER_PROMPTS, and the absence of a column is a stronger guarantee
-- than the absence of a write.
-- ---------------------------------------------------------------------------
CREATE TABLE prompt (
    prompt_id      TEXT PRIMARY KEY,
    session_id     TEXT,
    ts             INTEGER,
    prompt_length  INTEGER,
    command_name   TEXT,
    command_source TEXT
);

CREATE INDEX prompt_session ON prompt (session_id, ts DESC);

-- ---------------------------------------------------------------------------
-- permission_mode_change — feeds the Phase 6 risk rules.
-- ---------------------------------------------------------------------------
CREATE TABLE permission_mode_change (
    id         INTEGER PRIMARY KEY,
    session_id TEXT,
    from_mode  TEXT,
    to_mode    TEXT,
    trigger    TEXT,
    ts         INTEGER
);

CREATE INDEX permission_mode_change_session ON permission_mode_change (session_id, ts);

-- ---------------------------------------------------------------------------
-- Full-text search.
--
-- External-content FTS5 over tool_call: the text is not stored twice, and
-- snippet()/highlight() still work, which contentless mode cannot offer and
-- Phase 5 needs for match highlighting.
-- ---------------------------------------------------------------------------
CREATE VIRTUAL TABLE tool_call_fts USING fts5 (
    tool_name,
    input_summary,
    target_path,
    result_text,
    content = 'tool_call',
    content_rowid = 'rowid',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER tool_call_fts_insert AFTER INSERT ON tool_call BEGIN
    INSERT INTO tool_call_fts (rowid, tool_name, input_summary, target_path, result_text)
    VALUES (new.rowid, new.tool_name, new.input_summary, new.target_path, new.result_text);
END;

CREATE TRIGGER tool_call_fts_delete AFTER DELETE ON tool_call BEGIN
    INSERT INTO tool_call_fts (tool_call_fts, rowid, tool_name, input_summary, target_path, result_text)
    VALUES ('delete', old.rowid, old.tool_name, old.input_summary, old.target_path, old.result_text);
END;

CREATE TRIGGER tool_call_fts_update AFTER UPDATE ON tool_call BEGIN
    INSERT INTO tool_call_fts (tool_call_fts, rowid, tool_name, input_summary, target_path, result_text)
    VALUES ('delete', old.rowid, old.tool_name, old.input_summary, old.target_path, old.result_text);
    INSERT INTO tool_call_fts (rowid, tool_name, input_summary, target_path, result_text)
    VALUES (new.rowid, new.tool_name, new.input_summary, new.target_path, new.result_text);
END;
