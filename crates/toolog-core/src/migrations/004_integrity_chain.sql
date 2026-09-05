-- Migration 004 — the integrity hash chain over `raw_event` (task 7.6).
--
-- Each stored record carries the hash of a digest of itself linked to the hash
-- of the record before it, so editing any row — its body, the file it came
-- from, when it was ingested, or its position in the sequence — breaks every
-- chain value after it. `toolog verify --chain` walks it.
--
-- The column is nullable because rows written before this migration have no
-- chain value yet. Sealing them is a Rust pass over the existing rows in id
-- order (`chain::seal`), run by the process that owns the write connection;
-- SQLite has no SHA-256 of its own, so this cannot be done in SQL.
ALTER TABLE raw_event ADD COLUMN chain_sha256 TEXT;

-- Finding the tail of the chain, and finding what is not yet sealed, are the
-- two questions asked of this column.
CREATE INDEX raw_event_unchained ON raw_event (id) WHERE chain_sha256 IS NULL;
