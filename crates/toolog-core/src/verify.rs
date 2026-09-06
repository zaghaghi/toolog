//! What the store can and cannot account for (task 7.1).
//!
//! This is the module that lets the tool state its own completeness rather than
//! assume it, and it is the one that most distinguishes an audit trail from a
//! log viewer. A log viewer shows what it has. An audit trail also says what it
//! is missing, and when.
//!
//! # What a lane's absence means
//!
//! [ADR-0009] originally read an OTEL-only call as a **rejected** call. Phase 4
//! disproved that by measuring a real denial and finding it in *both* lanes: a
//! refused call still leaves a `tool_use` block and a `tool_result` whose body
//! is the refusal message. So rejections are counted from the `decision` column
//! and never from a missing lane, and the lanes mean what they say:
//!
//! - **Both** — the complete record: what ran, and who approved it.
//! - **Transcript only** — the OTLP lane was not running or not configured when
//!   this ran. We know what the agent did and not who let it. This is a gap in
//!   the *decision* layer, and [`Gap`] reports the windows it falls in.
//! - **OTEL only** — no transcript body was ever written. On the owner's store
//!   this is zero, and it should be: it would mean a transcript deleted or
//!   never flushed, which is a gap in the *content* layer and the more serious
//!   of the two, because the body is the evidence.
//!
//! [ADR-0009]: ../../../docs/adr/0009-correlate-on-tool-use-id.md

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::model::Reconciliation;

/// A stretch of time whose calls the OTLP lane never witnessed.
///
/// Contiguous in call order rather than clock time, and machine-wide rather
/// than per-session, because "capture was not running" is a fact about the
/// machine: two sessions overlapping the same window are inside the same gap.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Gap {
    pub from_ms: i64,
    pub to_ms: i64,
    /// Calls inside it with no decision layer.
    pub calls: i64,
    /// Distinct sessions those calls belong to.
    pub sessions: i64,
}

impl Gap {
    /// How long the gap lasted. Zero for a single call.
    #[must_use]
    pub fn duration_ms(&self) -> i64 {
        self.to_ms - self.from_ms
    }
}

/// One session's share of the record.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct SessionCompleteness {
    pub session_id: String,
    pub project_path: Option<String>,
    pub calls: i64,
    /// Calls the OTLP lane witnessed — the ones whose approval is on record.
    pub decided: i64,
    pub rejected: i64,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
}

impl SessionCompleteness {
    /// The fraction of this session's calls whose approval is on record.
    ///
    /// Deliberately not called "completeness" on its own: the content layer is
    /// complete for every one of these calls. What this measures is how much of
    /// the *decision* layer survives.
    #[must_use]
    pub fn decided_ratio(&self) -> Option<f64> {
        ratio(self.decided, self.calls)
    }
}

/// Everything `toolog verify` reports about the store's coverage.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Completeness {
    /// The lane counts, unchanged from [`crate::query::reconcile`].
    pub lanes: Reconciliation,
    /// Sessions, least complete first — the order this is read in.
    pub sessions: Vec<SessionCompleteness>,
    /// Windows with no decision layer, largest first.
    pub gaps: Vec<Gap>,
    /// Sessions in the store. `sessions` is capped, so the two differ when
    /// there are more than the report names.
    pub sessions_total: i64,
    /// Sessions missing part of their approval record, whether named or not.
    pub sessions_incomplete: i64,
    /// Calls with a timestamp, which gap detection needs. A call with none is
    /// counted in the lanes and left out of the windows rather than guessed at.
    pub placed: i64,
    /// `api_request` records held, the OTLP lane's other half.
    ///
    /// Counted and never rendered as spend: toolog does not report cost
    /// ([ADR-0010]). The count is here because a lane nothing displays is a
    /// lane that can stop arriving unnoticed, and this report exists to say
    /// what is missing.
    ///
    /// [ADR-0010]: ../../../docs/adr/0010-no-cost-reporting.md
    pub api_requests: i64,
}

impl Completeness {
    /// The fraction of all calls whose approval is on record.
    #[must_use]
    pub fn decided_ratio(&self) -> Option<f64> {
        let total = self.lanes.total();
        ratio(self.lanes.both + self.lanes.otel_only, total)
    }

    /// Whether every call the store holds carries both lanes.
    #[must_use]
    pub fn whole(&self) -> bool {
        self.lanes.total() > 0 && self.lanes.transcript_only == 0 && self.lanes.otel_only == 0
    }
}

fn ratio(part: i64, whole: i64) -> Option<f64> {
    (whole > 0).then(|| {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a ratio for display; these counts are far below 2^53"
        )]
        let r = part as f64 / whole as f64;
        r
    })
}

/// How many sessions the report names before it stops.
const SESSION_LIMIT: u32 = 50;
/// How many gaps it names.
const GAP_LIMIT: u32 = 20;

/// Reconcile the lanes, per session and over time.
pub fn completeness(conn: &Connection) -> Result<Completeness> {
    let (sessions_total, sessions_incomplete) = conn.query_row(
        "SELECT count(*), sum(incomplete) FROM (
             SELECT sum(CASE WHEN provenance & 2 != 0 THEN 0 ELSE 1 END) > 0 AS incomplete
             FROM tool_call WHERE session_id IS NOT NULL
             GROUP BY session_id)",
        [],
        |r| Ok((r.get(0)?, r.get::<_, Option<i64>>(1)?.unwrap_or_default())),
    )?;

    Ok(Completeness {
        lanes: crate::query::reconcile(conn)?,
        sessions: sessions(conn)?,
        gaps: gaps(conn)?,
        sessions_total,
        sessions_incomplete,
        placed: conn.query_row(
            "SELECT count(*) FROM tool_call WHERE called_at IS NOT NULL",
            [],
            |r| r.get(0),
        )?,
        api_requests: conn.query_row("SELECT count(*) FROM api_request", [], |r| r.get(0))?,
    })
}

/// Per-session coverage, least complete first.
///
/// Ties break on size, so the session most worth explaining is the one at the
/// top: a wholly unwitnessed session of 900 calls outranks one of three.
fn sessions(conn: &Connection) -> Result<Vec<SessionCompleteness>> {
    let mut stmt = conn.prepare(
        "SELECT tc.session_id, s.project_path,
                count(*) AS calls,
                sum(CASE WHEN tc.provenance & 2 != 0 THEN 1 ELSE 0 END) AS decided,
                sum(CASE WHEN tc.decision = 'reject' THEN 1 ELSE 0 END) AS rejected,
                min(tc.called_at), max(tc.called_at)
         FROM tool_call tc
         LEFT JOIN session s ON s.session_id = tc.session_id
         WHERE tc.session_id IS NOT NULL
         GROUP BY tc.session_id
         ORDER BY (CAST(decided AS REAL) / calls), calls DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([SESSION_LIMIT], |r| {
        Ok(SessionCompleteness {
            session_id: r.get(0)?,
            project_path: r.get(1)?,
            calls: r.get(2)?,
            decided: r.get::<_, Option<i64>>(3)?.unwrap_or_default(),
            rejected: r.get::<_, Option<i64>>(4)?.unwrap_or_default(),
            first_at: r.get(5)?,
            last_at: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Contiguous runs of calls with no decision layer.
///
/// The gaps-and-islands trick: number the calls in time order, number the
/// undecided ones separately, and the difference is constant within a run. It
/// is one pass, which matters because this runs over every call in the store.
fn gaps(conn: &Connection) -> Result<Vec<Gap>> {
    let mut stmt = conn.prepare(
        "WITH ordered AS (
             SELECT called_at, session_id,
                    provenance & 2 != 0 AS decided,
                    row_number() OVER (ORDER BY called_at, rowid) AS seq
             FROM tool_call
             WHERE called_at IS NOT NULL
         ),
         runs AS (
             SELECT called_at, session_id,
                    seq - row_number() OVER (ORDER BY called_at, seq) AS run
             FROM ordered WHERE decided = 0
         )
         SELECT min(called_at), max(called_at), count(*), count(DISTINCT session_id)
         FROM runs
         GROUP BY run
         ORDER BY count(*) DESC, min(called_at)
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([GAP_LIMIT], |r| {
        Ok(Gap {
            from_ms: r.get(0)?,
            to_ms: r.get(1)?,
            calls: r.get(2)?,
            sessions: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::model::{OtelFacts, Session, TranscriptFacts};
    use crate::project;

    /// A call the transcript saw. `decided` adds the OTLP lane on top.
    fn call(conn: &Connection, id: &str, session_id: &str, at: i64, decided: bool) {
        project::upsert_transcript(
            conn,
            id,
            &TranscriptFacts {
                session_id: Some(session_id.to_string()),
                tool_name: Some("Bash".to_string()),
                called_at: Some(at),
                success: Some(true),
                ..TranscriptFacts::default()
            },
        )
        .expect("transcript");
        if decided {
            project::upsert_otel(
                conn,
                id,
                &OtelFacts {
                    session_id: Some(session_id.to_string()),
                    decision: Some("accept".to_string()),
                    decision_source: Some("config".to_string()),
                    ..OtelFacts::default()
                },
            )
            .expect("otel");
        }
    }

    fn store() -> Db {
        let db = Db::open_in_memory().expect("open");
        let conn = db.conn();
        for (id, path) in [("old", "/work/alpha"), ("new", "/work/beta")] {
            project::upsert_session(
                conn,
                &Session {
                    session_id: id.to_string(),
                    project_path: Some(path.to_string()),
                    ..Session::default()
                },
            )
            .expect("session");
        }

        // Three undecided calls, then two decided, then two undecided again:
        // two windows in which nothing was watching.
        call(conn, "a1", "old", 1_000, false);
        call(conn, "a2", "old", 2_000, false);
        call(conn, "a3", "old", 3_000, false);
        call(conn, "b1", "new", 4_000, true);
        call(conn, "b2", "new", 5_000, true);
        call(conn, "b3", "new", 6_000, false);
        call(conn, "b4", "new", 7_000, false);
        db
    }

    #[test]
    fn a_session_reports_how_much_of_its_approval_layer_survives() {
        let db = store();
        let report = completeness(db.conn()).expect("completeness");

        assert_eq!(report.lanes.both, 2);
        assert_eq!(report.lanes.transcript_only, 5);
        assert_eq!(report.lanes.otel_only, 0);

        // Least complete first: `old` has nothing witnessed at all.
        let names: Vec<_> = report
            .sessions
            .iter()
            .map(|s| s.session_id.as_str())
            .collect();
        assert_eq!(names, ["old", "new"]);
        assert_eq!((report.sessions_total, report.sessions_incomplete), (2, 2));

        let old = &report.sessions[0];
        assert_eq!(old.calls, 3);
        assert_eq!(old.decided, 0);
        assert_eq!(old.decided_ratio(), Some(0.0));
        assert_eq!(old.project_path.as_deref(), Some("/work/alpha"));

        let new = &report.sessions[1];
        assert_eq!((new.calls, new.decided), (4, 2));
        assert_eq!(new.decided_ratio(), Some(0.5));
    }

    #[test]
    fn the_windows_where_nothing_was_watching_are_named() {
        let db = store();
        let report = completeness(db.conn()).expect("completeness");

        assert_eq!(report.gaps.len(), 2, "two runs of undecided calls");
        // Largest first.
        assert_eq!(report.gaps[0].calls, 3);
        assert_eq!(
            (report.gaps[0].from_ms, report.gaps[0].to_ms),
            (1_000, 3_000)
        );
        assert_eq!(report.gaps[0].duration_ms(), 2_000);
        assert_eq!(report.gaps[0].sessions, 1);

        assert_eq!(report.gaps[1].calls, 2);
        assert_eq!(
            (report.gaps[1].from_ms, report.gaps[1].to_ms),
            (6_000, 7_000)
        );
    }

    #[test]
    fn a_fully_witnessed_store_has_no_gaps_and_says_so() {
        let db = Db::open_in_memory().expect("open");
        call(db.conn(), "x1", "s", 1_000, true);
        call(db.conn(), "x2", "s", 2_000, true);

        let report = completeness(db.conn()).expect("completeness");
        assert!(report.whole());
        assert_eq!(report.decided_ratio(), Some(1.0));
        assert!(report.gaps.is_empty());
        assert_eq!((report.sessions_total, report.sessions_incomplete), (1, 0));
    }

    #[test]
    fn an_empty_store_is_not_complete_it_is_empty() {
        let db = Db::open_in_memory().expect("open");
        let report = completeness(db.conn()).expect("completeness");
        assert!(!report.whole(), "nothing to be complete about");
        assert_eq!(report.decided_ratio(), None, "and no ratio to report");
        assert!(report.sessions.is_empty() && report.gaps.is_empty());
    }

    /// A refusal is read from `decision`, so it is inside the decided count
    /// rather than being inferred from a missing lane.
    #[test]
    fn a_refusal_counts_as_witnessed_because_that_is_what_it_is() {
        let db = Db::open_in_memory().expect("open");
        let conn = db.conn();
        call(conn, "r1", "s", 1_000, true);
        project::upsert_otel(
            conn,
            "r1",
            &OtelFacts {
                decision: Some("reject".to_string()),
                ..OtelFacts::default()
            },
        )
        .expect("refusal");

        let report = completeness(conn).expect("completeness");
        assert_eq!(report.lanes.rejected, 1);
        assert_eq!(report.sessions[0].rejected, 1);
        assert_eq!(report.sessions[0].decided, 1);
        assert!(report.gaps.is_empty(), "a refusal is not a collection gap");
    }

    #[test]
    fn a_call_with_no_timestamp_is_counted_but_not_placed_in_a_window() {
        let db = Db::open_in_memory().expect("open");
        let conn = db.conn();
        call(conn, "t1", "s", 1_000, false);
        project::upsert_otel(
            conn,
            "no-time",
            &OtelFacts {
                session_id: Some("s".to_string()),
                decision: Some("accept".to_string()),
                ..OtelFacts::default()
            },
        )
        .expect("timeless");

        let report = completeness(conn).expect("completeness");
        assert_eq!(report.lanes.total(), 2);
        assert_eq!(report.placed, 1, "only one call can be put on a timeline");
        assert_eq!(report.gaps.len(), 1);
        assert_eq!(report.gaps[0].calls, 1);
    }
}
