//! Phase 1 exit criteria, end to end.
//!
//! A fixture set of raw events is inserted, projected, queried, wiped, and
//! re-projected — and must land on exactly the same rows. That round trip is
//! what [ADR-0004] promises: when a parser is fixed or Claude Code changes
//! format, history is re-derived from evidence rather than re-read from files
//! that may have rotated.
//!
//! The `FixtureProjector` below is deliberately *not* the Phase 2 normalizer.
//! It reads a simple test format, and exists to exercise the projection seam.

use rusqlite::Connection;
use serde::Deserialize;
use toolog_core::db::Db;
use toolog_core::model::{
    Lane, NewRawEvent, OtelFacts, Page, RawEvent, Session, TimelineFilter, TranscriptFacts,
    provenance,
};
use toolog_core::project::{self, Projector};
use toolog_core::{query, raw};

// ---------------------------------------------------------------------------
// A minimal projector over a test-only record format.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
enum Fixture {
    #[serde(rename = "transcript_tool_call")]
    TranscriptToolCall {
        tool_use_id: String,
        session_id: String,
        tool_name: String,
        input_summary: String,
        target_path: Option<String>,
        called_at: i64,
        success: bool,
        result_text: Option<String>,
    },
    #[serde(rename = "otel_tool_result")]
    OtelToolResult {
        tool_use_id: String,
        session_id: String,
        duration_ms: i64,
        decision: String,
        decision_source: String,
    },
    /// Anything the projector does not understand must be skipped, never fatal.
    #[serde(other)]
    Unknown,
}

#[derive(Default)]
struct FixtureProjector {
    skipped: usize,
}

impl Projector for FixtureProjector {
    fn project(&mut self, conn: &Connection, event: &RawEvent) -> toolog_core::Result<()> {
        let Ok(fixture) = serde_json::from_str::<Fixture>(&event.body) else {
            self.skipped += 1;
            return Ok(());
        };

        match fixture {
            Fixture::TranscriptToolCall {
                tool_use_id,
                session_id,
                tool_name,
                input_summary,
                target_path,
                called_at,
                success,
                result_text,
            } => {
                project::upsert_session(
                    conn,
                    &Session {
                        session_id: session_id.clone(),
                        project_path: Some("/proj".into()),
                        first_seen: Some(called_at),
                        last_seen: Some(called_at),
                        ..Session::default()
                    },
                )?;
                project::upsert_transcript(
                    conn,
                    &tool_use_id,
                    &TranscriptFacts {
                        session_id: Some(session_id),
                        tool_name: Some(tool_name),
                        input_summary: Some(input_summary),
                        target_path,
                        called_at: Some(called_at),
                        success: Some(success),
                        result_text,
                        ..TranscriptFacts::default()
                    },
                )?;
            }
            Fixture::OtelToolResult {
                tool_use_id,
                session_id,
                duration_ms,
                decision,
                decision_source,
            } => {
                project::upsert_otel(
                    conn,
                    &tool_use_id,
                    &OtelFacts {
                        session_id: Some(session_id),
                        duration_ms: Some(duration_ms),
                        decision: Some(decision),
                        decision_source: Some(decision_source),
                        ..OtelFacts::default()
                    },
                )?;
            }
            Fixture::Unknown => self.skipped += 1,
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Three accepted calls, one rejected (OTLP only, as a denial really arrives),
/// one call the OTLP lane never saw, and one record from a future Claude Code
/// version that this projector cannot read.
const FIXTURES: &[&str] = &[
    r#"{"kind":"transcript_tool_call","tool_use_id":"toolu_1","session_id":"s1","tool_name":"Bash","input_summary":"cargo test --workspace","target_path":null,"called_at":1000,"success":true,"result_text":"test result: ok. 19 passed"}"#,
    r#"{"kind":"otel_tool_result","tool_use_id":"toolu_1","session_id":"s1","duration_ms":42,"decision":"accept","decision_source":"config"}"#,
    r#"{"kind":"transcript_tool_call","tool_use_id":"toolu_2","session_id":"s1","tool_name":"Edit","input_summary":"edit src/main.rs","target_path":"/proj/src/main.rs","called_at":2000,"success":true,"result_text":"applied"}"#,
    r#"{"kind":"otel_tool_result","tool_use_id":"toolu_2","session_id":"s1","duration_ms":7,"decision":"accept","decision_source":"user_temporary"}"#,
    r#"{"kind":"transcript_tool_call","tool_use_id":"toolu_3","session_id":"s1","tool_name":"Read","input_summary":"read /etc/hosts","target_path":"/etc/hosts","called_at":3000,"success":false,"result_text":"ENOENT"}"#,
    // Rejected: OTLP saw the decision, no transcript body exists.
    r#"{"kind":"otel_tool_result","tool_use_id":"toolu_4","session_id":"s1","duration_ms":0,"decision":"reject","decision_source":"user_reject"}"#,
    // A record type from a version this build predates.
    r#"{"kind":"quantum-entangled-worktree","payload":{"unknown":true}}"#,
];

/// `FIXTURES.len()` as an `i64`, for comparing against row counts.
fn fixture_count() -> i64 {
    i64::try_from(FIXTURES.len()).expect("fixture count fits")
}

fn seed(db: &Db) -> usize {
    let events: Vec<NewRawEvent<'_>> = FIXTURES
        .iter()
        .enumerate()
        .map(|(i, body)| NewRawEvent {
            lane: if body.contains("otel") {
                Lane::Otlp
            } else {
                Lane::Transcript
            },
            source_ref: "fixtures",
            source_offset: Some(i64::try_from(i).expect("fixture index fits")),
            body,
        })
        .collect();
    raw::insert_batch(db.conn(), &events).expect("seed")
}

/// A canonical dump of every projection table, for comparing before and after a
/// rebuild.
fn dump_projections(conn: &Connection) -> String {
    let mut out = String::new();
    for table in [
        "session",
        "tool_call",
        "file_change",
        "api_request",
        "prompt",
    ] {
        let sql = format!("SELECT * FROM {table}");
        let mut stmt = conn.prepare(&sql).expect("prepare dump");
        let n = stmt.column_count();
        let rows = stmt
            .query_map([], |row| {
                let mut cells = Vec::with_capacity(n);
                for i in 0..n {
                    cells.push(format!("{:?}", row.get::<_, rusqlite::types::Value>(i)?));
                }
                Ok(cells.join("|"))
            })
            .expect("dump rows");
        let mut lines: Vec<String> = rows.map(|r| r.expect("row")).collect();
        lines.sort();
        out.push_str(table);
        out.push('\n');
        out.push_str(&lines.join("\n"));
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn insert_project_query_wipe_reproject_is_stable() {
    let db = Db::open_in_memory().expect("open");
    assert_eq!(seed(&db), FIXTURES.len());

    let mut projector = FixtureProjector::default();
    let first = project::reproject(db.conn(), None, &mut projector).expect("project");
    assert_eq!(first.scanned, FIXTURES.len());
    assert_eq!(
        first.tool_calls, 4,
        "three transcript calls plus one rejection"
    );
    assert_eq!(
        projector.skipped, 1,
        "the unknown record is skipped, not fatal"
    );

    let before = dump_projections(db.conn());

    // Wipe and rebuild from evidence alone.
    let mut again = FixtureProjector::default();
    let second = project::reproject(db.conn(), None, &mut again).expect("re-project");
    let after = dump_projections(db.conn());

    assert_eq!(first.tool_calls, second.tool_calls);
    assert_eq!(before, after, "re-projection must reproduce identical rows");

    // raw_event is untouched by a rebuild — it is the evidence, not a cache.
    assert_eq!(
        raw::count(db.conn(), None).expect("raw count"),
        fixture_count()
    );
}

#[test]
fn reconciliation_separates_rejections_from_gaps() {
    let db = Db::open_in_memory().expect("open");
    seed(&db);
    project::reproject(db.conn(), None, &mut FixtureProjector::default()).expect("project");

    let r = query::reconcile(db.conn()).expect("reconcile");
    assert_eq!(r.both, 2, "toolu_1 and toolu_2 were seen by both lanes");
    assert_eq!(
        r.transcript_only, 1,
        "toolu_3: the OTLP lane missed it — a gap"
    );
    assert_eq!(
        r.otel_only, 1,
        "toolu_4: no transcript body was written for it"
    );
    assert_eq!(r.total(), 4);

    // Completeness measures transcript-witnessed calls, so the rejection does
    // not count against it.
    let completeness = r.completeness().expect("some calls");
    assert!(
        (completeness - 2.0 / 3.0).abs() < 1e-9,
        "got {completeness}"
    );
}

#[test]
fn a_rejected_call_is_visible_with_no_transcript_body() {
    let db = Db::open_in_memory().expect("open");
    seed(&db);
    project::reproject(db.conn(), None, &mut FixtureProjector::default()).expect("project");

    let call = query::tool_call_detail(db.conn(), "toolu_4")
        .expect("query")
        .expect("rejected call is present");

    assert!(call.is_rejected());
    assert!(call.from_otel() && !call.from_transcript());
    assert_eq!(call.provenance, provenance::OTLP);
    assert_eq!(call.decision_source.as_deref(), Some("user_reject"));
    assert!(
        call.input_json.is_none() && call.input_summary.is_none(),
        "a denied call has no transcript content — that is the point"
    );
}

/// The core correctness property of ADR-0009.
#[test]
fn lane_arrival_order_does_not_change_the_row() {
    fn build(otel_first: bool) -> toolog_core::model::ToolCall {
        let db = Db::open_in_memory().expect("open");
        let transcript = TranscriptFacts {
            session_id: Some("s1".into()),
            tool_name: Some("Bash".into()),
            input_summary: Some("a very long command that OTEL would truncate".into()),
            called_at: Some(1000),
            success: Some(true),
            ..TranscriptFacts::default()
        };
        let otel = OtelFacts {
            session_id: Some("s1".into()),
            duration_ms: Some(99),
            decision: Some("accept".into()),
            decision_source: Some("config".into()),
            ..OtelFacts::default()
        };

        if otel_first {
            project::upsert_otel(db.conn(), "toolu_x", &otel).expect("otel");
            project::upsert_transcript(db.conn(), "toolu_x", &transcript).expect("transcript");
        } else {
            project::upsert_transcript(db.conn(), "toolu_x", &transcript).expect("transcript");
            project::upsert_otel(db.conn(), "toolu_x", &otel).expect("otel");
        }

        query::tool_call_detail(db.conn(), "toolu_x")
            .expect("query")
            .expect("row")
    }

    let a = build(true);
    let b = build(false);

    assert_eq!(a.provenance, provenance::BOTH);
    assert_eq!(
        format!("{a:?}"),
        format!("{b:?}"),
        "arrival order must not matter"
    );
    assert_eq!(a.duration_ms, Some(99));
    assert_eq!(
        a.input_summary.as_deref(),
        Some("a very long command that OTEL would truncate"),
        "the OTLP lane must never overwrite transcript content"
    );
}

#[test]
fn timeline_filters_and_paging() {
    let db = Db::open_in_memory().expect("open");
    seed(&db);
    project::reproject(db.conn(), None, &mut FixtureProjector::default()).expect("project");

    let all = query::timeline_page(db.conn(), &TimelineFilter::default(), Page::default())
        .expect("timeline");
    assert_eq!(all.len(), 4);
    assert!(
        all[0].called_at >= all[1].called_at,
        "newest first: {:?}",
        all.iter().map(|c| c.called_at).collect::<Vec<_>>()
    );

    let bash = TimelineFilter {
        tool_name: Some("Bash".into()),
        ..TimelineFilter::default()
    };
    assert_eq!(query::timeline_count(db.conn(), &bash).expect("count"), 1);

    let failed = TimelineFilter {
        success: Some(false),
        ..TimelineFilter::default()
    };
    let rows = query::timeline_page(db.conn(), &failed, Page::default()).expect("failed");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tool_use_id, "toolu_3");

    // All four, including the rejection: OTLP supplies its session_id, so a
    // denied call is still attributed to the project it was denied in — which
    // is exactly what a risk review needs to see.
    let in_project = TimelineFilter {
        project_path: Some("/proj".into()),
        ..TimelineFilter::default()
    };
    assert_eq!(
        query::timeline_count(db.conn(), &in_project).expect("count"),
        4
    );

    // Exact, not a mask: the whole point of the lane filter is to separate
    // "both lanes agree" from "only one lane ever saw this".
    let both_lanes = TimelineFilter {
        provenance: Some(provenance::BOTH),
        ..TimelineFilter::default()
    };
    assert_eq!(
        query::timeline_count(db.conn(), &both_lanes).expect("count"),
        2
    );

    let otel_only = TimelineFilter {
        provenance: Some(provenance::OTLP),
        ..TimelineFilter::default()
    };
    assert_eq!(
        query::timeline_count(db.conn(), &otel_only).expect("count"),
        1,
        "one call had no transcript body written for it"
    );

    let page = Page {
        limit: 2,
        offset: 0,
    };
    assert_eq!(
        query::timeline_page(db.conn(), &TimelineFilter::default(), page)
            .expect("page")
            .len(),
        2
    );
}

#[test]
fn search_matches_ranks_and_survives_shell_syntax() {
    let db = Db::open_in_memory().expect("open");
    seed(&db);
    project::reproject(db.conn(), None, &mut FixtureProjector::default()).expect("project");

    let hits = query::search(db.conn(), "cargo", Page::default()).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].tool_call.tool_use_id, "toolu_1");
    assert!(
        hits[0].snippet.contains(query::MATCH_OPEN),
        "snippet marks the match"
    );

    // Result text is indexed too, not just the command.
    assert_eq!(
        query::search(db.conn(), "ENOENT", Page::default())
            .expect("search")
            .len(),
        1
    );

    // Paths are searchable by segment.
    assert_eq!(
        query::search(db.conn(), "hosts", Page::default())
            .expect("search")
            .len(),
        1
    );

    // Blank input means "no filter applied", not "every row".
    assert!(
        query::search(db.conn(), "   ", Page::default())
            .expect("search")
            .is_empty()
    );

    // The real hazard: every one of these is valid FTS5 syntax and a plausible
    // thing to type into a search box over a corpus of shell commands.
    for hostile in [
        "rm -rf", "a | b", "*.rs", "--force", "NOT", "AND", "\"", "^x", "a:b", "(",
    ] {
        query::search(db.conn(), hostile, Page::default())
            .unwrap_or_else(|e| panic!("search({hostile:?}) must not error: {e}"));
    }
}

#[test]
fn stats_report_totals_and_latency() {
    let db = Db::open_in_memory().expect("open");
    seed(&db);
    project::reproject(db.conn(), None, &mut FixtureProjector::default()).expect("project");

    let totals = query::stats_totals(db.conn()).expect("totals");
    assert_eq!(totals.raw_events, fixture_count());
    assert_eq!(totals.tool_calls, 4);
    assert_eq!(totals.sessions, 1);
    assert_eq!(
        totals.cost_usd_micros, 0,
        "no OTLP api_request events in this fixture"
    );

    let usage = query::stats_tool_usage(db.conn()).expect("usage");
    let read = usage
        .iter()
        .find(|u| u.tool_name == "Read")
        .expect("Read row");
    assert_eq!(read.calls, 1);
    assert_eq!(read.failures, 1);

    let bash = usage
        .iter()
        .find(|u| u.tool_name == "Bash")
        .expect("Bash row");
    assert_eq!(bash.p50_ms, Some(42), "latency comes from the OTLP lane");
}

/// WAL exists so the UI can read while ingestion writes. If this ever blocks,
/// the timeline stalls behind a backfill.
#[test]
fn wal_lets_a_reader_work_during_an_open_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("wal.db");

    let writer = Db::open(&path).expect("writer");
    seed(&writer);
    project::reproject(writer.conn(), None, &mut FixtureProjector::default()).expect("project");

    let reader = Connection::open(&path).expect("reader");
    reader
        .busy_timeout(std::time::Duration::from_millis(500))
        .expect("timeout");

    let before: i64 = reader
        .query_row("SELECT count(*) FROM tool_call", [], |r| r.get(0))
        .expect("read before");
    assert_eq!(before, 4);

    // Hold a write transaction open across the reads.
    writer
        .conn()
        .execute_batch("BEGIN IMMEDIATE")
        .expect("begin");
    project::upsert_transcript(
        writer.conn(),
        "toolu_wal",
        &TranscriptFacts {
            tool_name: Some("Write".into()),
            called_at: Some(9000),
            ..TranscriptFacts::default()
        },
    )
    .expect("write in tx");

    // The reader sees the committed snapshot and is not blocked by the writer.
    let during: i64 = reader
        .query_row("SELECT count(*) FROM tool_call", [], |r| r.get(0))
        .expect("read during an open write");
    assert_eq!(during, 4, "uncommitted work must not be visible");

    writer.conn().execute_batch("COMMIT").expect("commit");

    let after: i64 = reader
        .query_row("SELECT count(*) FROM tool_call", [], |r| r.get(0))
        .expect("read after");
    assert_eq!(after, 5);
}

/// A refusal is read from the decision, never from a missing transcript.
///
/// Phase 4 measured a live denial and found it in **both** lanes: the
/// transcript keeps the `tool_use` block and a `tool_result` holding the
/// refusal message. Inferring rejection from provenance — as ADR-0009
/// originally proposed — would have missed every one of them.
#[test]
fn a_refused_call_is_counted_even_when_both_lanes_saw_it() {
    use toolog_core::model::{OtelFacts, TranscriptFacts};

    let db = Db::open_in_memory().expect("open");

    project::upsert_transcript(
        db.conn(),
        "toolu_denied",
        &TranscriptFacts {
            tool_name: Some("Bash".to_string()),
            input_summary: Some("ls /nowhere".to_string()),
            result_text: Some("Permission to use Bash has been denied".to_string()),
            ..TranscriptFacts::default()
        },
    )
    .expect("transcript");
    project::upsert_otel(
        db.conn(),
        "toolu_denied",
        &OtelFacts {
            tool_name: Some("Bash".to_string()),
            decision: Some("reject".to_string()),
            decision_source: Some("config".to_string()),
            ..OtelFacts::default()
        },
    )
    .expect("otel");

    let r = query::reconcile(db.conn()).expect("reconcile");
    assert_eq!(r.both, 1, "a denial is witnessed by both lanes");
    assert_eq!(r.otel_only, 0, "provenance says nothing about refusal");
    assert_eq!(r.rejected, 1, "the decision column is what identifies it");

    let call = query::tool_call_detail(db.conn(), "toolu_denied")
        .expect("query")
        .expect("row");
    assert!(call.is_rejected());
    assert_eq!(
        call.decision_source.as_deref(),
        Some("config"),
        "which rule denied it lives only in the OTLP lane"
    );
}

/// The assumption ADR-0011's memo rests on, asserted rather than described.
///
/// The risk review is cached in memory and retired by `PRAGMA data_version`.
/// That is only sound if the pragma moves for every write that could change a
/// finding — and, just as importantly, if it does **not** move for the writing
/// connection's own write, which is why the review reads it on a connection of
/// its own.
#[test]
fn data_version_moves_for_another_connections_writes_and_not_for_its_own() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("watermark.db");
    let writer = Db::open(&path).expect("writer");
    let reader = Db::open(&path).expect("reader");
    let (w, r) = (writer.conn(), reader.conn());

    let version = |c: &Connection| -> i64 {
        c.query_row("PRAGMA data_version", [], |row| row.get(0))
            .expect("pragma")
    };

    let start = version(r);
    assert_eq!(version(r), start, "reading it does not move it");

    let insert = |id: &str, at: i64| {
        project::upsert_transcript(
            w,
            id,
            &TranscriptFacts {
                session_id: Some("s".into()),
                tool_name: Some("Bash".into()),
                called_at: Some(at),
                ..TranscriptFacts::default()
            },
        )
        .expect("insert");
    };

    insert("t1", 1_000);
    let after_insert = version(r);
    assert_ne!(after_insert, start, "an insert by the writer moves it");

    // The case `max(rowid)` misses: the OTEL lane completing a row the
    // transcript created, which is the arrival that adds the `decision` most
    // of the risk rules read (ADR-0009).
    project::upsert_otel(
        w,
        "t1",
        &OtelFacts {
            decision: Some("reject".into()),
            ..OtelFacts::default()
        },
    )
    .expect("otel");
    let after_update = version(r);
    assert_ne!(
        after_update, after_insert,
        "an update of an existing row moves it"
    );

    // The case the writer's update hook misses: it ignores SQLITE_DELETE.
    w.execute("DELETE FROM tool_call WHERE tool_use_id = 't1'", [])
        .expect("delete");
    assert_ne!(version(r), after_update, "a delete moves it");

    // And the property that makes a separate read connection necessary rather
    // than merely tidy: a connection never sees its own writes here, so a memo
    // guarded on the writing connection would never expire.
    let before_own = version(w);
    insert("t2", 2_000);
    assert_eq!(
        version(w),
        before_own,
        "a connection's own write must not move its own data_version"
    );
}
