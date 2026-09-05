//! The integrity chain over `raw_event` (task 7.6).
//!
//! Every stored record carries `chain_sha256`, the hash of a digest of that
//! record linked to the chain value of the record before it. Change anything
//! about a row — its body, the file it came from, when it was ingested, or its
//! place in the order — and its own chain value no longer recomputes, and
//! neither does any value after it. [`verify`] walks the chain and says where
//! it first breaks.
//!
//! # What this proves, and what it does not
//!
//! It detects **modification of stored evidence**. It does not prove the
//! evidence was **complete** when it was written — a record that was never
//! captured leaves nothing to break. That is what `toolog verify`'s
//! reconciliation is for (task 7.1), and the two answer different questions.
//!
//! It also does not stop an adversary who can write to the database and knows
//! this file exists: they can edit a row and recompute every chain value after
//! it. Nothing stored *inside* the thing being protected can prevent that. What
//! makes the chain worth having is that its head is a single 64-character
//! string covering everything before it, so **recording the head somewhere else
//! turns a silent rewrite into a visible one**. `toolog verify --chain` prints
//! it for exactly that reason.
//!
//! The two checks cover different tampering, and neither subsumes the other:
//!
//! - **The walk** catches any edit that leaves the rest of the chain in place —
//!   a changed body, a changed `source_ref`, a deleted or reordered record.
//! - **The head** catches a rewrite that re-seals everything after the edit,
//!   which the walk cannot, because such a chain is internally consistent.
//!
//! Deleting a record from the *middle* leaves the head untouched, because the
//! last record's stored value did not change. So keeping the head is not a
//! substitute for walking, and walking is not a substitute for keeping the
//! head.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension as _, params};
use sha2::{Digest, Sha256};

use crate::error::Result;

/// What the first record's chain value is linked to.
///
/// A fixed string rather than an empty one, so a chain of a single row is
/// still the hash of something named, and so an empty database has a defined
/// head rather than `None` meaning two different things.
pub const GENESIS: &str = "toolog:chain:v1";

/// The digest of one record's own contents.
///
/// Every column of `raw_event` that carries meaning is in here. `content_sha256`
/// stands for the body — it is checked against the body separately, so a body
/// edited without updating its hash is caught by [`verify`] as well.
///
/// The separator cannot appear in any field (all are hex, integers, or paths),
/// so two different records cannot produce the same digest by rearranging where
/// one field ends and the next begins.
#[must_use]
pub fn row_digest(
    lane: &str,
    source_ref: &str,
    source_offset: Option<i64>,
    ingested_at: i64,
    content_sha256: &str,
) -> String {
    let offset = source_offset.map_or_else(|| "-".to_string(), |o| o.to_string());
    hash(&format!(
        "{lane}\n{source_ref}\n{offset}\n{ingested_at}\n{content_sha256}"
    ))
}

/// One link: the previous chain value, then this record's digest.
#[must_use]
pub fn link(previous: &str, digest: &str) -> String {
    hash(&format!("{previous}\n{digest}"))
}

fn hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}

/// The chain value of the last sealed record, or [`GENESIS`] if there is none.
///
/// This is the number to write down. Anyone holding an older head can prove the
/// records it covered have not changed since.
pub fn head(conn: &Connection) -> Result<String> {
    Ok(conn
        .query_row(
            "SELECT chain_sha256 FROM raw_event
             WHERE chain_sha256 IS NOT NULL
             ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .unwrap_or_else(|| GENESIS.to_string()))
}

/// How many records are stored but not yet part of the chain.
pub fn unsealed(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT count(*) FROM raw_event WHERE chain_sha256 IS NULL",
        [],
        |r| r.get(0),
    )?)
}

/// What sealing did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    /// Records added to the chain by this call.
    pub rows: i64,
    /// The chain head afterwards.
    pub head: String,
}

/// Add every unchained record to the chain, in id order.
///
/// Idempotent, and cheap when there is nothing to do — one indexed count. Runs
/// where writes already happen: the process that owns the write connection
/// calls it at startup, and a backfill calls it when it finishes. A record is
/// sealed once and never re-sealed, so this cannot rewrite history it has
/// already committed to.
pub fn seal(conn: &Connection) -> Result<Sealed> {
    let mut previous = head(conn)?;
    let mut rows = 0i64;

    let pending: Vec<(i64, String, String, Option<i64>, i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, lane, source_ref, source_offset, ingested_at, content_sha256
             FROM raw_event WHERE chain_sha256 IS NULL ORDER BY id",
        )?;
        let found = stmt.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?;
        found.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut update = conn
        .prepare("UPDATE raw_event SET chain_sha256 = ?2 WHERE id = ?1 AND chain_sha256 IS NULL")?;
    for (id, lane, source_ref, source_offset, ingested_at, content_sha256) in pending {
        let digest = row_digest(
            &lane,
            &source_ref,
            source_offset,
            ingested_at,
            &content_sha256,
        );
        let value = link(&previous, &digest);
        update.execute(params![id, &value])?;
        previous = value;
        rows += 1;
    }

    Ok(Sealed {
        rows,
        head: previous,
    })
}

/// Where the chain stops being true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Break {
    /// The `raw_event` row that does not check out.
    pub id: i64,
    /// Which of the two checks failed, in words.
    pub what: String,
    /// The purge that accounts for it, if one does.
    ///
    /// Removing records breaks the chain, and it should — something changed.
    /// A purge records what it removed and the chain value it removed it
    /// after ([`crate::retention`]), so a break at the far side of that hole is
    /// explained rather than alarming. `None` is the one worth looking at.
    pub explained_by: Option<String>,
}

/// The result of walking the whole chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainReport {
    /// Records checked — those with a chain value.
    pub checked: i64,
    /// Records stored but never sealed. Not a failure: a store upgraded from
    /// an earlier version has these until something writes to it.
    pub unsealed: i64,
    /// The head after the last checked record. Worth recording elsewhere.
    pub head: String,
    /// Every record that failed a check, in id order.
    pub breaks: Vec<Break>,
}

impl ChainReport {
    /// Whether every sealed record checked out.
    #[must_use]
    pub fn intact(&self) -> bool {
        self.breaks.is_empty()
    }

    /// Whether every break is accounted for by a recorded purge.
    ///
    /// The question a script should ask, rather than [`ChainReport::intact`]:
    /// a store with a retention policy breaks its own chain on purpose, and
    /// treating that as tampering would make the check unusable on any store
    /// that bounds itself.
    #[must_use]
    pub fn accounted_for(&self) -> bool {
        self.breaks.iter().all(|b| b.explained_by.is_some())
    }

    /// The breaks nothing explains.
    #[must_use]
    pub fn unexplained(&self) -> Vec<&Break> {
        self.breaks
            .iter()
            .filter(|b| b.explained_by.is_none())
            .collect()
    }
}

/// Walk the chain, recomputing both hashes for every sealed record.
///
/// Two checks per record, and they catch different things:
///
/// 1. `content_sha256` against the body — an edited body.
/// 2. `chain_sha256` against the previous value and this record's digest — an
///    edited `source_ref` or `ingested_at`, a deleted record, a reordered one,
///    or a record inserted after the fact.
///
/// It reports every break rather than stopping at the first, so a store with
/// several independent edits shows all of them rather than one at a time.
pub fn verify(conn: &Connection) -> Result<ChainReport> {
    let mut stmt = conn.prepare(
        "SELECT id, lane, source_ref, source_offset, ingested_at, content_sha256, chain_sha256, body
         FROM raw_event WHERE chain_sha256 IS NOT NULL ORDER BY id",
    )?;

    // What each recorded purge removed records *after*, so a break at the far
    // side of one of those holes can be named rather than merely reported.
    let holes: HashMap<String, String> = crate::retention::deletions(conn)?
        .into_iter()
        .filter_map(|d| d.chain_before.map(|c| (c, d.detail)))
        .collect();

    let mut previous = GENESIS.to_string();
    let mut report = ChainReport {
        checked: 0,
        unsealed: unsealed(conn)?,
        head: GENESIS.to_string(),
        breaks: Vec::new(),
    };

    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let lane: String = row.get(1)?;
        let source_ref: String = row.get(2)?;
        let source_offset: Option<i64> = row.get(3)?;
        let ingested_at: i64 = row.get(4)?;
        let content_sha256: String = row.get(5)?;
        let stored_chain: String = row.get(6)?;
        let body: String = row.get(7)?;

        if crate::raw::content_hash(&body) != content_sha256 {
            report.breaks.push(Break {
                id,
                what: "the body does not match its content hash".to_string(),
                // A purge removes records; it never edits one. So this kind of
                // break is never explained by a deletion.
                explained_by: None,
            });
        }

        let digest = row_digest(
            &lane,
            &source_ref,
            source_offset,
            ingested_at,
            &content_sha256,
        );
        let expected = link(&previous, &digest);
        if expected != stored_chain {
            report.breaks.push(Break {
                id,
                what: "the chain value does not follow from the record before it".to_string(),
                explained_by: holes.get(&previous).cloned(),
            });
        }

        // Continue from what is *stored*, not from what was expected. Otherwise
        // one altered row would cascade and report every row after it as
        // broken, burying the one that actually changed under thousands that
        // did not. One edit is one break.
        previous = stored_chain;
        report.checked += 1;
    }

    report.head = previous;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::model::{Lane, NewRawEvent};
    use crate::raw;

    fn store(bodies: &[&str]) -> Db {
        let db = Db::open_in_memory().expect("open");
        for (i, body) in bodies.iter().enumerate() {
            raw::insert(
                db.conn(),
                &NewRawEvent {
                    lane: Lane::Transcript,
                    source_ref: "s.jsonl",
                    source_offset: Some(i64::try_from(i).unwrap_or(0)),
                    body,
                },
            )
            .expect("insert");
        }
        db
    }

    #[test]
    fn every_stored_record_is_chained_as_it_is_written() {
        let db = store(&["{\"a\":1}", "{\"a\":2}", "{\"a\":3}"]);
        assert_eq!(unsealed(db.conn()).expect("unsealed"), 0);

        let report = verify(db.conn()).expect("verify");
        assert!(report.intact());
        assert_eq!(report.checked, 3);
        assert_eq!(report.head, head(db.conn()).expect("head"));
        assert_ne!(report.head, GENESIS);
    }

    #[test]
    fn an_empty_store_has_the_genesis_head_rather_than_nothing() {
        let db = Db::open_in_memory().expect("open");
        assert_eq!(head(db.conn()).expect("head"), GENESIS);
        let report = verify(db.conn()).expect("verify");
        assert!(report.intact());
        assert_eq!(report.checked, 0);
    }

    /// The phase's exit criterion: a row edited with the `sqlite3` CLI is
    /// detected. This is that edit, made the same way.
    #[test]
    fn an_edited_body_is_detected() {
        let db = store(&["{\"a\":1}", "{\"a\":2}", "{\"a\":3}"]);
        db.conn()
            .execute("UPDATE raw_event SET body = '{\"a\":99}' WHERE id = 2", [])
            .expect("tamper");

        let report = verify(db.conn()).expect("verify");
        assert!(!report.intact());
        assert_eq!(report.breaks.len(), 1, "one row changed, one break");
        assert_eq!(report.breaks[0].id, 2);
        assert!(report.breaks[0].what.contains("content hash"));
    }

    #[test]
    fn editing_a_records_metadata_is_detected_too() {
        let db = store(&["{\"a\":1}", "{\"a\":2}"]);
        db.conn()
            .execute(
                "UPDATE raw_event SET source_ref = 'somewhere-else.jsonl' WHERE id = 1",
                [],
            )
            .expect("tamper");

        let report = verify(db.conn()).expect("verify");
        assert_eq!(report.breaks.len(), 1);
        assert_eq!(report.breaks[0].id, 1);
        assert!(report.breaks[0].what.contains("does not follow"));
    }

    #[test]
    fn deleting_a_record_from_the_middle_is_detected_by_the_walk_not_the_head() {
        let db = store(&["{\"a\":1}", "{\"a\":2}", "{\"a\":3}"]);
        let head_before = head(db.conn()).expect("head");

        db.conn()
            .execute("DELETE FROM raw_event WHERE id = 2", [])
            .expect("tamper");

        let report = verify(db.conn()).expect("verify");
        assert!(!report.intact(), "a hole in the sequence breaks the chain");
        assert_eq!(report.breaks[0].id, 3, "the record that now follows id 1");
        assert_eq!(
            report.head, head_before,
            "and the head is unchanged, because the last record was not touched \
             — which is why keeping the head is not a substitute for the walk"
        );
    }

    /// The difference the report is meant to show: one edit is one break, and
    /// a rewrite from some point onward is none at all — which is why the head
    /// has to be kept somewhere else.
    #[test]
    fn a_rewritten_chain_is_internally_consistent_and_the_head_gives_it_away() {
        let db = store(&["{\"a\":1}", "{\"a\":2}", "{\"a\":3}"]);
        let head_before = head(db.conn()).expect("head");

        // Edit a body, fix its content hash, then re-seal everything after it,
        // which is what a thorough tamperer would do.
        db.conn()
            .execute(
                "UPDATE raw_event SET body = '{\"a\":99}',
                        content_sha256 = ?1,
                        chain_sha256 = NULL
                 WHERE id = 2",
                params![raw::content_hash("{\"a\":99}")],
            )
            .expect("tamper");
        db.conn()
            .execute("UPDATE raw_event SET chain_sha256 = NULL WHERE id > 2", [])
            .expect("unseal the tail");
        seal(db.conn()).expect("re-seal");

        let report = verify(db.conn()).expect("verify");
        assert!(
            report.intact(),
            "a rewritten chain checks out against itself — nothing inside the \
             database can say otherwise"
        );
        assert_ne!(
            report.head, head_before,
            "the head is what gives it away, which is why it is printed"
        );
    }

    #[test]
    fn sealing_is_idempotent_and_leaves_earlier_links_alone() {
        let db = store(&["{\"a\":1}", "{\"a\":2}"]);
        let first = head(db.conn()).expect("head");

        let again = seal(db.conn()).expect("seal");
        assert_eq!(again.rows, 0, "nothing left to seal");
        assert_eq!(again.head, first);
    }

    #[test]
    fn records_written_before_the_chain_existed_are_sealed_in_order() {
        let db = store(&["{\"a\":1}", "{\"a\":2}", "{\"a\":3}"]);
        let sealed_normally = head(db.conn()).expect("head");

        // A store as it looks straight after migration 004.
        db.conn()
            .execute("UPDATE raw_event SET chain_sha256 = NULL", [])
            .expect("unseal");
        assert_eq!(unsealed(db.conn()).expect("unsealed"), 3);

        let sealed = seal(db.conn()).expect("seal");
        assert_eq!(sealed.rows, 3);
        assert_eq!(
            sealed.head, sealed_normally,
            "sealing in bulk produces the same chain as sealing as they arrive"
        );
        assert!(verify(db.conn()).expect("verify").intact());
    }
}
