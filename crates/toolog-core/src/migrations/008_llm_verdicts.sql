-- Migration 008 — what a local model said about a call (task 13.13).
--
-- ADR-0013. A verdict is **stored, not recomputed**, and that is a departure
-- from ADR-0004 that has to be argued rather than assumed.
--
-- ADR-0004 says the projection is a derivation: throw it away, re-run it, get
-- the same answer. Findings are computed for exactly that reason. An LLM answer
-- is not a derivation in that sense — a different model, quantization, sampler
-- seed or prompt gives a different number, and this build cannot even promise
-- the same answer twice from the same file, because tokenization depends on how
-- the prompt was split. So it is not a projection of the store. It is closer to
-- `rule_dismissal`: a judgement, recorded with what made it.
--
-- What makes it safe to store is the key. `(tool_use_id, model_fingerprint,
-- prompt_fingerprint)` is the same shape as `rule_sighting`'s (rule_id,
-- fingerprint, tool_use_id) and for the same reason: change the model or the
-- prompt and you are asking a *different question*, so the old answers stay as
-- true statements about what the old question got, and the new question starts
-- empty. Nothing here can go stale, because nothing here claims to be current.
--
-- `model_fingerprint` is the SHA-256 of the model **file** (task 13.14), never
-- its name. Two files called `gemma.gguf` are not the same model.
--
-- **No foreign key to `tool_call`, and never purged**, exactly as `rule_sighting`
-- and `deletion` are not. "A model examined this before you deleted it" is a
-- thing an audit trail should still be able to say. `retention.rs` gets no
-- DELETE for this table.
CREATE TABLE llm_verdict (
    tool_use_id       TEXT    NOT NULL,
    -- SHA-256 of the .gguf that answered.
    model_fingerprint TEXT    NOT NULL,
    -- SHA-256 of the rendered instructions and the grammar together.
    prompt_fingerprint TEXT   NOT NULL,

    -- 'ok' or 'failed'. A verdict that fails schema validation is recorded as
    -- failed rather than dropped (task 13.10): "asked and could not answer" and
    -- "never asked" are different facts, and a backfill that silently skipped a
    -- call would report the second while meaning the first. It is also what
    -- stops a call that always fails being retried on every pass forever.
    status            TEXT    NOT NULL CHECK (status IN ('ok', 'failed')),
    -- Why, when status is 'failed'. NULL otherwise.
    error             TEXT,

    -- All NULL when status is 'failed'.
    risk_score        INTEGER CHECK (risk_score IS NULL OR risk_score BETWEEN 1 AND 5),
    category          TEXT,
    intent_summary    TEXT,
    is_destructive    INTEGER,
    violates_sandbox  INTEGER,

    -- When the verdict was recorded, and how long the model took. `ms` is kept
    -- because task 13.20 owes a measured number and a benchmark that runs only
    -- on the author's machine is not one.
    at                INTEGER NOT NULL,
    ms                INTEGER NOT NULL,

    PRIMARY KEY (tool_use_id, model_fingerprint, prompt_fingerprint)
) WITHOUT ROWID;

-- "The highest-scoring commands no rule matched" is the risk view's one new
-- section (task 13.16), and it is this index.
CREATE INDEX llm_verdict_score
    ON llm_verdict (model_fingerprint, prompt_fingerprint, risk_score DESC);

-- The backfill's own question, asked once per batch: which calls has this
-- (model, prompt) pair not answered for yet.
CREATE INDEX llm_verdict_pair ON llm_verdict (model_fingerprint, prompt_fingerprint, tool_use_id);

-- `@intent:` is full-text over the summaries (task 13.15).
--
-- Its own FTS table rather than a column on `tool_call_fts`: a call has one row
-- in `tool_call` and may have many verdicts, one per (model, prompt) pair, so
-- there is no column on `tool_call` for this to be. An external-content table
-- keyed on `llm_verdict`'s rowid is not available either — the table is
-- WITHOUT ROWID, deliberately, because its key is three text columns. So this
-- is a contentless-adjacent index maintained by triggers over the one column
-- worth searching, with the primary key carried alongside so a hit can be
-- joined back.
CREATE VIRTUAL TABLE llm_verdict_fts USING fts5 (
    intent_summary,
    tool_use_id UNINDEXED,
    model_fingerprint UNINDEXED,
    prompt_fingerprint UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER llm_verdict_fts_insert AFTER INSERT ON llm_verdict
WHEN new.intent_summary IS NOT NULL BEGIN
    INSERT INTO llm_verdict_fts (intent_summary, tool_use_id, model_fingerprint, prompt_fingerprint)
    VALUES (new.intent_summary, new.tool_use_id, new.model_fingerprint, new.prompt_fingerprint);
END;

CREATE TRIGGER llm_verdict_fts_delete AFTER DELETE ON llm_verdict BEGIN
    DELETE FROM llm_verdict_fts
     WHERE tool_use_id = old.tool_use_id
       AND model_fingerprint = old.model_fingerprint
       AND prompt_fingerprint = old.prompt_fingerprint;
END;

-- A verdict is written once and replaced only by a re-run over the same pair,
-- which `INSERT OR REPLACE` performs as a delete and an insert — so the two
-- triggers above already cover it. This one covers an in-place UPDATE, which
-- nothing does today and which would otherwise leave the index behind.
CREATE TRIGGER llm_verdict_fts_update AFTER UPDATE ON llm_verdict BEGIN
    DELETE FROM llm_verdict_fts
     WHERE tool_use_id = old.tool_use_id
       AND model_fingerprint = old.model_fingerprint
       AND prompt_fingerprint = old.prompt_fingerprint;
    INSERT INTO llm_verdict_fts (intent_summary, tool_use_id, model_fingerprint, prompt_fingerprint)
    SELECT new.intent_summary, new.tool_use_id, new.model_fingerprint, new.prompt_fingerprint
     WHERE new.intent_summary IS NOT NULL;
END;
