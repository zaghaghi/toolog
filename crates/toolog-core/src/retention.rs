//! Bounding the store, and removing what someone asks to be rid of (tasks 7.4
//! and 7.8).
//!
//! # Nothing is deleted without a preview
//!
//! Every entry point is a pair: [`preview`] says exactly what would go, and
//! [`purge`] does it. The CLI previews by default and needs `--apply` to
//! proceed, because "I ran the retention command to see what it would do" must
//! not be how an audit trail loses a year of history.
//!
//! # A session is the unit
//!
//! Evidence and projection have no row-level link — a `tool_call` does not
//! record which `raw_event` produced it. What they share is the session: a
//! transcript file *is* a session, and `session.transcript_path` names it. So
//! retention picks whole sessions and removes both halves together, which is
//! the only way to avoid the two states worth avoiding: evidence whose
//! projection is gone (and which a re-projection would silently bring back),
//! and projections whose evidence is gone (which can never be rebuilt).
//!
//! The OTLP lane has no such file. Its records are written as they happen, so
//! their ingestion time *is* their event time, and they are removed by the same
//! cutoff directly.
//!
//! # The chain breaks, visibly
//!
//! Removing records from `raw_event` breaks the integrity chain over them, and
//! it is supposed to: something did change. What a purge must not do is leave
//! that unexplained, so it writes a [`Deletion`] first, and
//! [`crate::chain::verify`] reports a break as accounted for or not. See
//! [`crate::chain`] for what that does and does not prove.

use rusqlite::{Connection, OptionalExtension as _, params};
use serde::{Deserialize, Serialize};

use crate::chain;
use crate::error::Result;

/// What to remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[ts(export_to = "unused/")]
pub enum Scope {
    /// Sessions whose last activity is before this instant.
    Before { cutoff_ms: i64 },
    /// The oldest sessions, until the stored bytes fit inside a cap.
    ToFit { max_bytes: i64 },
    /// One session, named.
    Session { session_id: String },
    /// Every session of one project.
    Project { project_path: String },
}

impl Scope {
    /// The word that goes in the deletion record's `reason`.
    fn reason(&self) -> &'static str {
        match self {
            Self::Before { .. } | Self::ToFit { .. } => "retention",
            Self::Session { .. } => "session",
            Self::Project { .. } => "project",
        }
    }
}

/// One session that a purge would remove.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Doomed {
    pub session_id: String,
    pub project_path: Option<String>,
    pub transcript_path: Option<String>,
    pub tool_calls: i64,
    pub raw_events: i64,
    /// Stored bytes of those records — what removing them actually frees.
    pub bytes: i64,
    pub first_seen: Option<i64>,
    pub last_seen: Option<i64>,
}

/// Exactly what a purge would remove, before anything is removed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Preview {
    /// The scope in words, for the confirmation line.
    pub description: String,
    /// Every session that would go, oldest first.
    pub sessions: Vec<Doomed>,
    pub tool_calls: i64,
    pub raw_events: i64,
    /// Records of the OTLP lane removed by the cutoff, which belong to no file.
    pub otlp_records: i64,
    pub bytes: i64,
    /// What the store holds now, for "x of y".
    pub total_sessions: i64,
    pub total_tool_calls: i64,
    /// The cutoff a size cap resolved to. `None` for the other scopes.
    pub derived_cutoff_ms: Option<i64>,
}

impl Preview {
    /// Whether this would remove anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty() && self.otlp_records == 0
    }
}

/// What a purge removed. The same shape as the row it writes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Deletion {
    pub id: i64,
    pub at: i64,
    pub reason: String,
    pub detail: String,
    pub raw_events: i64,
    pub tool_calls: i64,
    pub sessions: i64,
    pub first_id: Option<i64>,
    pub last_id: Option<i64>,
    pub chain_before: Option<String>,
}

/// The sessions a scope names, oldest first.
fn doomed(conn: &Connection, scope: &Scope) -> Result<Vec<Doomed>> {
    // One shape of query with a different `WHERE`, so every scope reports the
    // same numbers and none can drift from the others.
    let (clause, bind): (&str, Box<dyn rusqlite::ToSql>) = match scope {
        Scope::Before { cutoff_ms } => (
            "COALESCE(s.last_seen, s.first_seen, 0) < ?1",
            Box::new(*cutoff_ms),
        ),
        // Resolved to a cutoff by the caller before it reaches here.
        Scope::ToFit { .. } => ("1 = 0", Box::new(0i64)),
        Scope::Session { session_id } => ("s.session_id = ?1", Box::new(session_id.clone())),
        Scope::Project { project_path } => ("s.project_path = ?1", Box::new(project_path.clone())),
    };

    let sql = format!(
        "SELECT s.session_id, s.project_path, s.transcript_path, s.first_seen, s.last_seen,
                (SELECT count(*) FROM tool_call tc WHERE tc.session_id = s.session_id),
                COALESCE((SELECT count(*) FROM raw_event r
                          WHERE r.source_ref = s.transcript_path), 0),
                COALESCE((SELECT sum(length(r.body)) FROM raw_event r
                          WHERE r.source_ref = s.transcript_path), 0)
         FROM session s
         WHERE {clause}
         ORDER BY COALESCE(s.last_seen, s.first_seen, 0), s.session_id"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([bind.as_ref()], |r| {
        Ok(Doomed {
            session_id: r.get(0)?,
            project_path: r.get(1)?,
            transcript_path: r.get(2)?,
            first_seen: r.get(3)?,
            last_seen: r.get(4)?,
            tool_calls: r.get(5)?,
            raw_events: r.get(6)?,
            bytes: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The cutoff that leaves the store inside `max_bytes`.
///
/// Walks sessions oldest first, accumulating what would be *kept*, and stops
/// at the first one that no longer fits. Only the transcript lane's bytes are
/// counted, because that is what a session owns; the total is an estimate for
/// the same reason `VACUUM` is a separate step — SQLite reclaims pages, not
/// bytes, and the two are not the same number.
fn cutoff_for(conn: &Connection, max_bytes: i64) -> Result<Option<i64>> {
    let all = doomed(
        conn,
        &Scope::Before {
            cutoff_ms: i64::MAX,
        },
    )?;
    let total: i64 = all.iter().map(|d| d.bytes).sum();
    if total <= max_bytes {
        return Ok(None);
    }

    // Drop oldest first until the rest fits.
    let mut dropped = 0i64;
    for session in &all {
        dropped += session.bytes;
        if total - dropped <= max_bytes {
            // Everything up to and including this session goes, so the cutoff
            // is one millisecond past its last activity.
            return Ok(Some(
                session.last_seen.or(session.first_seen).unwrap_or(0) + 1,
            ));
        }
    }
    Ok(Some(i64::MAX))
}

/// Say exactly what [`purge`] would remove.
pub fn preview(conn: &Connection, scope: &Scope) -> Result<Preview> {
    let (effective, derived) = match scope {
        Scope::ToFit { max_bytes } => match cutoff_for(conn, *max_bytes)? {
            Some(cutoff_ms) => (Scope::Before { cutoff_ms }, Some(cutoff_ms)),
            None => (Scope::Before { cutoff_ms: 0 }, None),
        },
        other => (other.clone(), None),
    };

    let sessions = doomed(conn, &effective)?;
    let otlp_records = match &effective {
        Scope::Before { cutoff_ms } => conn.query_row(
            "SELECT count(*) FROM raw_event WHERE lane = 'otlp' AND ingested_at < ?1",
            params![cutoff_ms],
            |r| r.get(0),
        )?,
        // Removing one session's OTLP records would mean knowing which they
        // are, and the lane records no file. Its projections go; its evidence
        // stays, and `toolog verify` shows the result as it always has.
        _ => 0i64,
    };

    Ok(Preview {
        description: describe(scope, derived),
        tool_calls: sessions.iter().map(|d| d.tool_calls).sum(),
        raw_events: sessions.iter().map(|d| d.raw_events).sum::<i64>() + otlp_records,
        bytes: sessions.iter().map(|d| d.bytes).sum(),
        otlp_records,
        total_sessions: conn.query_row("SELECT count(*) FROM session", [], |r| r.get(0))?,
        total_tool_calls: conn.query_row("SELECT count(*) FROM tool_call", [], |r| r.get(0))?,
        derived_cutoff_ms: derived,
        sessions,
    })
}

fn describe(scope: &Scope, derived: Option<i64>) -> String {
    match scope {
        Scope::Before { cutoff_ms } => {
            format!("sessions last active before {}", when(*cutoff_ms))
        }
        Scope::ToFit { max_bytes } => match derived {
            Some(cutoff) => format!(
                "the oldest sessions, to fit inside {max_bytes} bytes (before {})",
                when(cutoff)
            ),
            None => format!("nothing — the store already fits inside {max_bytes} bytes"),
        },
        Scope::Session { session_id } => format!("session {session_id}"),
        Scope::Project { project_path } => format!("every session in {project_path}"),
    }
}

/// A cutoff as a date rather than a number of milliseconds.
///
/// This string is stored in the deletion record and read back years later by
/// whoever is asking why the chain has a hole. `1783432128789` answers nothing.
fn when(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms).map_or_else(
        |_| ms.to_string(),
        |t| t.strftime("%Y-%m-%d %H:%M UTC").to_string(),
    )
}

/// Remove what [`preview`] described, and record that it happened.
///
/// Both halves in one transaction: evidence and projection go together or
/// neither does. The deletion record is written inside it too, so there is no
/// state in which records are gone and nothing says why.
///
/// Does **not** `VACUUM` — see [`vacuum`], which is separate because it
/// rewrites the whole file and the caller should decide when to pay for that.
pub fn purge(conn: &Connection, scope: &Scope) -> Result<Deletion> {
    let plan = preview(conn, scope)?;
    let tx = conn.unchecked_transaction()?;

    // The chain value the surviving record before the hole carries, recorded
    // before anything goes so a later break can be matched to this.
    let (first_id, last_id, chain_before) = hole(&tx, &plan)?;

    for session in &plan.sessions {
        tx.execute(
            "DELETE FROM tool_call WHERE session_id = ?1",
            params![session.session_id],
        )?;
        tx.execute(
            "DELETE FROM api_request WHERE session_id = ?1",
            params![session.session_id],
        )?;
        tx.execute(
            "DELETE FROM prompt WHERE session_id = ?1",
            params![session.session_id],
        )?;
        tx.execute(
            "DELETE FROM permission_mode_change WHERE session_id = ?1",
            params![session.session_id],
        )?;
        if let Some(path) = &session.transcript_path {
            tx.execute("DELETE FROM raw_event WHERE source_ref = ?1", params![path])?;
        }
        tx.execute(
            "DELETE FROM session WHERE session_id = ?1",
            params![session.session_id],
        )?;
    }

    if let (Scope::Before { .. } | Scope::ToFit { .. }, Some(cutoff)) = (scope, cutoff_of(&plan)) {
        tx.execute(
            "DELETE FROM raw_event WHERE lane = 'otlp' AND ingested_at < ?1",
            params![cutoff],
        )?;
    }

    let record = Deletion {
        id: 0,
        at: crate::raw::now_ms(),
        reason: scope.reason().to_string(),
        detail: plan.description.clone(),
        raw_events: plan.raw_events,
        tool_calls: plan.tool_calls,
        sessions: i64::try_from(plan.sessions.len()).unwrap_or(i64::MAX),
        first_id,
        last_id,
        chain_before,
    };
    tx.execute(
        "INSERT INTO deletion
             (at, reason, detail, raw_events, tool_calls, sessions,
              first_id, last_id, chain_before)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            record.at,
            record.reason,
            record.detail,
            record.raw_events,
            record.tool_calls,
            record.sessions,
            record.first_id,
            record.last_id,
            record.chain_before,
        ],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(Deletion { id, ..record })
}

/// The cutoff a plan resolved to, if it had one.
fn cutoff_of(plan: &Preview) -> Option<i64> {
    plan.derived_cutoff_ms.or_else(|| {
        plan.description
            .rsplit_once("before ")
            .and_then(|(_, n)| n.trim_end_matches(')').parse().ok())
    })
}

/// The `raw_event` id span about to be removed, and the chain value of the
/// record that will precede the hole.
fn hole(conn: &Connection, plan: &Preview) -> Result<(Option<i64>, Option<i64>, Option<String>)> {
    let paths: Vec<String> = plan
        .sessions
        .iter()
        .filter_map(|d| d.transcript_path.clone())
        .collect();
    if paths.is_empty() {
        return Ok((None, None, None));
    }

    let holes = vec!["?"; paths.len()].join(", ");
    let binds: Vec<&dyn rusqlite::ToSql> =
        paths.iter().map(|p| p as &dyn rusqlite::ToSql).collect();

    let sql = format!("SELECT min(id), max(id) FROM raw_event WHERE source_ref IN ({holes})");
    let (first, last): (Option<i64>, Option<i64>) =
        conn.query_row(&sql, binds.as_slice(), |r| Ok((r.get(0)?, r.get(1)?)))?;

    let chain_before = match first {
        Some(first) => Some(
            conn.query_row(
                "SELECT chain_sha256 FROM raw_event
                 WHERE id < ?1 AND chain_sha256 IS NOT NULL
                 ORDER BY id DESC LIMIT 1",
                params![first],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            // A hole at the very start has no record before it, and what the
            // walk will carry there is the genesis value — so that is what has
            // to be recorded, or the first purge a store ever makes is the one
            // break it cannot explain.
            .unwrap_or_else(|| chain::GENESIS.to_string()),
        ),
        None => None,
    };
    Ok((first, last, chain_before))
}

/// Every purge this store has recorded, newest first.
pub fn deletions(conn: &Connection) -> Result<Vec<Deletion>> {
    let mut stmt = conn.prepare(
        "SELECT id, at, reason, detail, raw_events, tool_calls, sessions,
                first_id, last_id, chain_before
         FROM deletion ORDER BY at DESC, id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Deletion {
            id: r.get(0)?,
            at: r.get(1)?,
            reason: r.get(2)?,
            detail: r.get(3)?,
            raw_events: r.get(4)?,
            tool_calls: r.get(5)?,
            sessions: r.get(6)?,
            first_id: r.get(7)?,
            last_id: r.get(8)?,
            chain_before: r.get(9)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Reclaim the space a purge freed, returning the bytes the file lost.
///
/// Separate from [`purge`] because it rewrites the entire database: on a
/// hundred-megabyte store that is seconds of blocked writes, and the caller
/// should choose when to spend them rather than discover it.
pub fn vacuum(conn: &Connection) -> Result<i64> {
    let before = file_bytes(conn)?;
    conn.execute_batch("VACUUM")?;
    Ok(before - file_bytes(conn)?)
}

/// The database's size, from SQLite rather than the filesystem — the WAL and
/// the page count are what change here, not the caller's idea of the path.
pub fn file_bytes(conn: &Connection) -> Result<i64> {
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok(pages * size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::model::{Lane, NewRawEvent, Session, TranscriptFacts};
    use crate::{project, raw};

    const DAY: i64 = 86_400_000;

    /// A session with `calls` calls, its transcript, and its evidence.
    fn session(db: &Db, id: &str, project: &str, last_seen: i64, calls: i64) {
        let conn = db.conn();
        let path = format!("/transcripts/{id}.jsonl");
        project::upsert_session(
            conn,
            &Session {
                session_id: id.to_string(),
                project_path: Some(project.to_string()),
                transcript_path: Some(path.clone()),
                first_seen: Some(last_seen - DAY),
                last_seen: Some(last_seen),
                ..Session::default()
            },
        )
        .expect("session");

        for n in 0..calls {
            raw::insert(
                conn,
                &NewRawEvent {
                    lane: Lane::Transcript,
                    source_ref: &path,
                    source_offset: Some(n),
                    body: &format!(r#"{{"session":"{id}","n":{n}}}"#),
                },
            )
            .expect("evidence");
            project::upsert_transcript(
                conn,
                &format!("{id}-{n}"),
                &TranscriptFacts {
                    session_id: Some(id.to_string()),
                    tool_name: Some("Bash".to_string()),
                    called_at: Some(last_seen),
                    ..TranscriptFacts::default()
                },
            )
            .expect("call");
        }
    }

    fn store() -> Db {
        let db = Db::open_in_memory().expect("open");
        session(&db, "old", "/work/alpha", 10 * DAY, 3);
        session(&db, "middle", "/work/beta", 50 * DAY, 2);
        session(&db, "new", "/work/alpha", 90 * DAY, 4);
        db
    }

    fn counts(conn: &Connection) -> (i64, i64, i64) {
        let one = |sql: &str| {
            conn.query_row(sql, [], |r| r.get::<_, i64>(0))
                .expect("count")
        };
        (
            one("SELECT count(*) FROM session"),
            one("SELECT count(*) FROM tool_call"),
            one("SELECT count(*) FROM raw_event"),
        )
    }

    #[test]
    fn a_preview_names_every_session_and_removes_nothing() {
        let db = store();
        let before = counts(db.conn());

        let plan = preview(
            db.conn(),
            &Scope::Before {
                cutoff_ms: 60 * DAY,
            },
        )
        .expect("preview");
        assert_eq!(
            plan.sessions
                .iter()
                .map(|d| d.session_id.as_str())
                .collect::<Vec<_>>(),
            ["old", "middle"],
            "oldest first, and only what is older than the cutoff"
        );
        assert_eq!(plan.tool_calls, 5);
        assert_eq!(plan.raw_events, 5);
        assert!(plan.bytes > 0);
        assert_eq!((plan.total_sessions, plan.total_tool_calls), (3, 9));

        assert_eq!(counts(db.conn()), before, "a preview deletes nothing");
    }

    #[test]
    fn a_purge_removes_both_halves_of_a_session_together() {
        let db = store();
        purge(
            db.conn(),
            &Scope::Before {
                cutoff_ms: 60 * DAY,
            },
        )
        .expect("purge");

        let (sessions, calls, events) = counts(db.conn());
        assert_eq!(sessions, 1, "only the newest survives");
        assert_eq!(calls, 4);
        assert_eq!(
            events, 4,
            "the evidence went with it — otherwise a re-projection would bring \
             the calls back"
        );
    }

    #[test]
    fn one_session_can_be_removed_by_name() {
        let db = store();
        let plan = preview(
            db.conn(),
            &Scope::Session {
                session_id: "middle".to_string(),
            },
        )
        .expect("preview");
        assert_eq!(plan.sessions.len(), 1);
        assert_eq!(plan.tool_calls, 2);

        purge(
            db.conn(),
            &Scope::Session {
                session_id: "middle".to_string(),
            },
        )
        .expect("purge");
        assert_eq!(counts(db.conn()), (2, 7, 7));
    }

    #[test]
    fn a_project_takes_every_session_it_had() {
        let db = store();
        purge(
            db.conn(),
            &Scope::Project {
                project_path: "/work/alpha".to_string(),
            },
        )
        .expect("purge");

        let (sessions, calls, _) = counts(db.conn());
        assert_eq!(
            (sessions, calls),
            (1, 2),
            "beta's session is all that is left"
        );
    }

    #[test]
    fn a_size_cap_drops_the_oldest_until_the_rest_fits() {
        let db = store();
        let everything = preview(
            db.conn(),
            &Scope::Before {
                cutoff_ms: i64::MAX,
            },
        )
        .expect("preview");
        let newest = everything
            .sessions
            .iter()
            .find(|d| d.session_id == "new")
            .expect("the newest session")
            .bytes;

        // A cap with room for the newest session and nothing more.
        let plan = preview(db.conn(), &Scope::ToFit { max_bytes: newest }).expect("preview");
        assert!(
            plan.derived_cutoff_ms.is_some(),
            "a cap resolves to a cutoff"
        );
        assert_eq!(
            plan.sessions
                .iter()
                .map(|d| d.session_id.as_str())
                .collect::<Vec<_>>(),
            ["old", "middle"],
            "the oldest go first, and only as many as the cap needs"
        );
    }

    /// A cap smaller than the newest session cannot keep it, and says so rather
    /// than quietly keeping something that does not fit.
    #[test]
    fn a_cap_too_small_for_anything_takes_everything() {
        let db = store();
        let plan = preview(db.conn(), &Scope::ToFit { max_bytes: 1 }).expect("preview");
        assert_eq!(plan.sessions.len(), 3);
        assert_eq!(plan.total_sessions, 3);
    }

    #[test]
    fn a_cap_the_store_already_fits_removes_nothing() {
        let db = store();
        let plan = preview(
            db.conn(),
            &Scope::ToFit {
                max_bytes: i64::MAX,
            },
        )
        .expect("preview");
        assert!(plan.is_empty());
        assert!(plan.description.contains("already fits"));
    }

    /// The chain is supposed to break. What matters is that it is explained.
    #[test]
    fn a_purge_records_what_it_removed_and_where_the_hole_is() {
        let db = store();
        assert!(chain::verify(db.conn()).expect("verify").intact());

        let record = purge(
            db.conn(),
            &Scope::Session {
                session_id: "old".to_string(),
            },
        )
        .expect("purge");

        assert_eq!(record.reason, "session");
        assert!(record.detail.contains("old"));
        assert_eq!(
            (record.tool_calls, record.raw_events, record.sessions),
            (3, 3, 1)
        );
        assert_eq!(record.first_id, Some(1), "the oldest evidence removed");

        let log = deletions(db.conn()).expect("deletions");
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].id, record.id);

        // The chain now reports a break, which is correct — and names the
        // purge that caused it, which is the difference between a retention
        // policy and tampering.
        let report = chain::verify(db.conn()).expect("verify");
        assert!(!report.intact(), "removing records changes the record");
        assert!(
            report.accounted_for(),
            "a break with no explanation: {:#?}",
            report.unexplained()
        );
        assert!(
            report.breaks[0]
                .explained_by
                .as_deref()
                .is_some_and(|why| why.contains("old")),
            "the break should name the purge that made it"
        );
    }

    /// The other half: an edit is *not* explained by a purge that happened to
    /// have occurred, or the accounting would launder tampering.
    #[test]
    fn a_purge_does_not_excuse_an_edit_made_afterwards() {
        let db = store();
        purge(
            db.conn(),
            &Scope::Session {
                session_id: "old".to_string(),
            },
        )
        .expect("purge");

        db.conn()
            .execute(
                "UPDATE raw_event SET body = body || ' ' WHERE id = (SELECT max(id) FROM raw_event)",
                [],
            )
            .expect("tamper");

        let report = chain::verify(db.conn()).expect("verify");
        assert!(
            !report.accounted_for(),
            "an edited body is not something a deletion can account for"
        );
        assert!(
            report
                .unexplained()
                .iter()
                .any(|b| b.what.contains("content hash"))
        );
    }

    #[test]
    fn the_deletion_record_survives_a_later_purge() {
        let db = store();
        purge(
            db.conn(),
            &Scope::Session {
                session_id: "old".to_string(),
            },
        )
        .expect("first");
        purge(
            db.conn(),
            &Scope::Before {
                cutoff_ms: i64::MAX,
            },
        )
        .expect("everything else");

        assert_eq!(counts(db.conn()).0, 0, "no sessions left");
        assert_eq!(
            deletions(db.conn()).expect("deletions").len(),
            2,
            "a retention policy that erased its own history would be useless"
        );
    }

    #[test]
    fn purging_nothing_is_not_an_error_and_still_says_so() {
        let db = store();
        let record = purge(db.conn(), &Scope::Before { cutoff_ms: 0 }).expect("purge");
        assert_eq!((record.sessions, record.tool_calls), (0, 0));
        assert_eq!(counts(db.conn()), (3, 9, 9));
    }

    #[test]
    fn search_still_works_after_a_purge() {
        let db = store();
        purge(
            db.conn(),
            &Scope::Session {
                session_id: "old".to_string(),
            },
        )
        .expect("purge");

        // The FTS index is external-content, so it only stays true because a
        // trigger removes each row as the row behind it goes. A search that
        // returned deleted calls would be worse than one that returned none.
        let hits =
            crate::query::search(db.conn(), "Bash", crate::model::Page::default()).expect("search");
        assert!(
            !hits.is_empty(),
            "the surviving sessions are still findable"
        );
        assert!(
            hits.iter()
                .all(|h| !h.tool_call.tool_use_id.starts_with("old-")),
            "the index still had the deleted session's rows"
        );
    }
}
