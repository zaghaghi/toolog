//! The evidence store: append-only writes to `raw_event`.
//!
//! [ADR-0004] in one module. Every input record is written here verbatim before
//! anything parses it, because Claude Code's formats drift and data lost at
//! ingestion cannot be recovered.
//!
//! [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::model::{Lane, NewRawEvent, RawEvent, RawInsert};

/// Milliseconds since the Unix epoch.
///
/// Public because "when did this happen" is asked outside the evidence store
/// too — a dismissal is stamped with it — and one clock reading in the
/// workspace is one fewer thing to get subtly different.
#[must_use]
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Content hash used for deduplication, and the anchor for the Phase 7
/// integrity chain.
#[must_use]
pub fn content_hash(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    hex::encode(hasher.finalize())
}

/// Store one record.
///
/// Returns [`RawInsert::Duplicate`] if an identical body is already held. That
/// is a normal outcome, not an error: it is what lets the Phase 2 tailer
/// recover from a truncated file by rescanning from zero.
///
/// The record is linked into the integrity chain in the same statement that
/// creates it ([`crate::chain`]), so there is no window in which a stored
/// record is outside the chain. Ordering is safe because one process holds one
/// write connection ([ADR-0007]) — the chain head cannot move between reading
/// it and using it.
///
/// [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md
pub fn insert(conn: &Connection, event: &NewRawEvent<'_>) -> Result<RawInsert> {
    // Off by default: the evidence keeps what the projection hides, so a
    // redaction pattern that turns out to be wrong can be fixed and the
    // projection rebuilt. Task 7.3, and `crate::redact::REDACT_EVIDENCE`.
    let body = crate::redact::evidence(event.body);
    let hash = content_hash(&body);
    let at = now_ms();
    let digest = crate::chain::row_digest(
        event.lane.as_str(),
        event.source_ref,
        event.source_offset,
        at,
        &hash,
    );
    let chained = crate::chain::link(&crate::chain::head(conn)?, &digest);

    let mut stmt = conn.prepare_cached(
        "INSERT INTO raw_event
             (lane, source_ref, source_offset, content_sha256, ingested_at, body, chain_sha256)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (content_sha256) DO NOTHING
         RETURNING id",
    )?;

    let id = stmt
        .query_row(
            params![
                event.lane.as_str(),
                event.source_ref,
                event.source_offset,
                hash,
                at,
                body.as_ref(),
                chained,
            ],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;

    Ok(match id {
        Some(id) => RawInsert::Inserted(id),
        None => RawInsert::Duplicate,
    })
}

/// Store many records in one transaction. Returns how many were new.
pub fn insert_batch(conn: &Connection, events: &[NewRawEvent<'_>]) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let mut inserted = 0;
    for event in events {
        if insert(&tx, event)?.is_new() {
            inserted += 1;
        }
    }
    tx.commit()?;
    Ok(inserted)
}

/// Count stored records, optionally for one lane.
pub fn count(conn: &Connection, lane: Option<Lane>) -> Result<i64> {
    Ok(match lane {
        Some(lane) => conn.query_row(
            "SELECT count(*) FROM raw_event WHERE lane = ?1",
            params![lane.as_str()],
            |r| r.get(0),
        )?,
        None => conn.query_row("SELECT count(*) FROM raw_event", [], |r| r.get(0))?,
    })
}

/// The highest byte offset recorded for `source_ref`, for resuming a tail.
pub fn max_offset(conn: &Connection, source_ref: &str) -> Result<Option<i64>> {
    Ok(conn.query_row(
        "SELECT max(source_offset) FROM raw_event WHERE source_ref = ?1",
        params![source_ref],
        |r| r.get::<_, Option<i64>>(0),
    )?)
}

/// Stream stored records in insertion order, oldest first.
///
/// This is the input to re-projection ([`crate::project::reproject`]).
pub fn scan(conn: &Connection, lane: Option<Lane>, f: &mut dyn FnMut(&RawEvent)) -> Result<usize> {
    let sql = "SELECT id, lane, source_ref, source_offset, content_sha256, ingested_at, body
               FROM raw_event
               WHERE (?1 IS NULL OR lane = ?1)
               ORDER BY id";
    let mut stmt = conn.prepare(sql)?;
    let lane = lane.map(Lane::as_str);

    let rows = stmt.query_map(params![lane], |row| {
        Ok(RawEvent {
            id: row.get(0)?,
            lane: row.get(1)?,
            source_ref: row.get(2)?,
            source_offset: row.get(3)?,
            content_sha256: row.get(4)?,
            ingested_at: row.get(5)?,
            body: row.get(6)?,
        })
    })?;

    let mut n = 0;
    for row in rows {
        f(&row?);
        n += 1;
    }
    Ok(n)
}

/// The highest `raw_event.id` currently stored.
///
/// Paired with [`scan_after`] this lets an incremental ingest project only the
/// records it just added.
pub fn max_id(conn: &Connection) -> Result<i64> {
    Ok(
        conn.query_row("SELECT COALESCE(max(id), 0) FROM raw_event", [], |r| {
            r.get(0)
        })?,
    )
}

/// Stream records stored after `after_id`, oldest first.
pub fn scan_after(conn: &Connection, after_id: i64, f: &mut dyn FnMut(&RawEvent)) -> Result<usize> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, lane, source_ref, source_offset, content_sha256, ingested_at, body
         FROM raw_event WHERE id > ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![after_id], |row| {
        Ok(RawEvent {
            id: row.get(0)?,
            lane: row.get(1)?,
            source_ref: row.get(2)?,
            source_offset: row.get(3)?,
            content_sha256: row.get(4)?,
            ingested_at: row.get(5)?,
            body: row.get(6)?,
        })
    })?;

    let mut n = 0;
    for row in rows {
        f(&row?);
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn event(body: &str, offset: i64) -> NewRawEvent<'_> {
        NewRawEvent {
            lane: Lane::Transcript,
            source_ref: "session.jsonl",
            source_offset: Some(offset),
            body,
        }
    }

    #[test]
    fn identical_bodies_are_stored_once() {
        let db = Db::open_in_memory().expect("open");
        let body = r#"{"type":"assistant","uuid":"a"}"#;

        assert!(insert(db.conn(), &event(body, 0)).expect("first").is_new());
        assert_eq!(
            insert(db.conn(), &event(body, 0)).expect("second"),
            RawInsert::Duplicate
        );
        assert_eq!(count(db.conn(), None).expect("count"), 1);
    }

    #[test]
    fn dedup_is_by_content_not_offset() {
        // The tailer rescans from zero after a truncation, so the same line can
        // legitimately arrive at a different offset. Content decides.
        let db = Db::open_in_memory().expect("open");
        let body = r#"{"type":"user"}"#;

        insert(db.conn(), &event(body, 0)).expect("first");
        assert_eq!(
            insert(db.conn(), &event(body, 4096)).expect("re-scan"),
            RawInsert::Duplicate
        );
        assert_eq!(count(db.conn(), None).expect("count"), 1);
    }

    #[test]
    fn batch_reports_only_new_rows() {
        let db = Db::open_in_memory().expect("open");
        let events = vec![
            event("{\"a\":1}", 0),
            event("{\"b\":2}", 1),
            event("{\"a\":1}", 2),
        ];

        assert_eq!(insert_batch(db.conn(), &events).expect("batch"), 2);
        assert_eq!(count(db.conn(), None).expect("count"), 2);
    }

    #[test]
    fn lanes_are_counted_separately() {
        let db = Db::open_in_memory().expect("open");
        insert(db.conn(), &event("{\"t\":1}", 0)).expect("transcript");
        insert(
            db.conn(),
            &NewRawEvent {
                lane: Lane::Otlp,
                source_ref: "batch-1",
                source_offset: None,
                body: "{\"o\":1}",
            },
        )
        .expect("otlp");

        assert_eq!(count(db.conn(), Some(Lane::Transcript)).expect("t"), 1);
        assert_eq!(count(db.conn(), Some(Lane::Otlp)).expect("o"), 1);
        assert_eq!(count(db.conn(), None).expect("all"), 2);
    }

    #[test]
    fn max_offset_supports_resuming_a_tail() {
        let db = Db::open_in_memory().expect("open");
        insert(db.conn(), &event("{\"a\":1}", 10)).expect("a");
        insert(db.conn(), &event("{\"b\":2}", 250)).expect("b");

        assert_eq!(
            max_offset(db.conn(), "session.jsonl").expect("max"),
            Some(250)
        );
        assert_eq!(max_offset(db.conn(), "absent.jsonl").expect("none"), None);
    }

    #[test]
    fn scan_yields_records_in_insertion_order() {
        let db = Db::open_in_memory().expect("open");
        insert(db.conn(), &event("{\"n\":1}", 0)).expect("1");
        insert(db.conn(), &event("{\"n\":2}", 1)).expect("2");

        let mut seen = Vec::new();
        scan(db.conn(), None, &mut |e| seen.push(e.body.clone())).expect("scan");
        assert_eq!(seen, vec!["{\"n\":1}", "{\"n\":2}"]);
    }

    #[test]
    fn body_is_stored_byte_for_byte() {
        // ADR-0004: verbatim. Whitespace, key order and unknown fields survive.
        let db = Db::open_in_memory().expect("open");
        let body = "{ \"weird\" :  [1,2,3] ,\"unknown_future_field\":true }";
        insert(db.conn(), &event(body, 0)).expect("insert");

        let stored: String = db
            .conn()
            .query_row("SELECT body FROM raw_event", [], |r| r.get(0))
            .expect("read back");
        assert_eq!(stored, body);
    }
}
