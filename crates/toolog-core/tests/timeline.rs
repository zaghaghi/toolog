//! The query surface the timeline view is built on (Phase 5).
//!
//! Phase 5's exit criterion is a sentence about behaviour — *find every
//! `rm -rf` ever run, narrow to one project, inspect one, export the set* — and
//! every clause of it is a query. These tests are that sentence, in the order
//! it is read.
//!
//! The corpus below is deliberately hostile in the way the real one is: shell
//! commands full of FTS5 operators, a session whose OTLP lane never arrived,
//! and subagent calls mixed into the main thread.

use toolog_core::db::Db;
use toolog_core::model::{OtelFacts, Page, Session, TimelineFilter, TranscriptFacts};
use toolog_core::{Connection, project, query};

/// A store holding two projects, three sessions and one subagent.
fn seeded() -> Db {
    let db = Db::open_in_memory().expect("open");
    let conn = db.conn();

    for (id, project, branch) in [
        ("s-app", "/work/app", "main"),
        ("s-app-2", "/work/app", "fix/login"),
        ("s-infra", "/work/infra", "main"),
    ] {
        project::upsert_session(
            conn,
            &Session {
                session_id: id.to_string(),
                project_path: Some(project.to_string()),
                git_branch: Some(branch.to_string()),
                cc_version: Some("2.1.260".to_string()),
                ..Session::default()
            },
        )
        .expect("session");
    }

    // The main thread of the first session.
    call(
        conn,
        "t1",
        "s-app",
        "Bash",
        "rm -rf target/debug",
        1_000,
        true,
    );
    call(
        conn,
        "t2",
        "s-app",
        "Bash",
        "cargo test --workspace",
        2_000,
        true,
    );
    call(conn, "t3", "s-app", "Edit", "src/main.rs", 3_000, true);

    // A subagent inside it: the same session, a separate group.
    agent_call(
        conn,
        "t4",
        "s-app",
        "a-1",
        "Explore",
        "grep -rn TODO",
        2_500,
    );
    agent_call(
        conn,
        "t5",
        "s-app",
        "a-1",
        "Explore",
        "rm -rf /tmp/x",
        2_600,
    );

    // A second session in the same project, and one in another.
    call(
        conn,
        "t6",
        "s-app-2",
        "Bash",
        "rm -rf node_modules",
        4_000,
        false,
    );
    call(
        conn,
        "t7",
        "s-infra",
        "Bash",
        "rm -rf .terraform",
        5_000,
        true,
    );

    // Only the first session's calls were witnessed by the OTLP lane.
    for (id, decision, source) in [
        ("t1", "accept", "user_temporary"),
        ("t2", "accept", "config"),
        ("t3", "accept", "config"),
    ] {
        project::upsert_otel(
            conn,
            id,
            &OtelFacts {
                session_id: Some("s-app".to_string()),
                duration_ms: Some(120),
                decision: Some(decision.to_string()),
                decision_source: Some(source.to_string()),
                ..OtelFacts::default()
            },
        )
        .expect("otel");
    }

    db
}

fn call(
    conn: &Connection,
    id: &str,
    session: &str,
    tool: &str,
    summary: &str,
    at: i64,
    success: bool,
) {
    project::upsert_transcript(
        conn,
        id,
        &TranscriptFacts {
            session_id: Some(session.to_string()),
            tool_name: Some(tool.to_string()),
            input_summary: Some(summary.to_string()),
            called_at: Some(at),
            success: Some(success),
            is_sidechain: Some(false),
            permission_mode: Some("default".to_string()),
            ..TranscriptFacts::default()
        },
    )
    .expect("transcript");
}

fn agent_call(
    conn: &Connection,
    id: &str,
    session: &str,
    agent_id: &str,
    agent_name: &str,
    summary: &str,
    at: i64,
) {
    project::upsert_transcript(
        conn,
        id,
        &TranscriptFacts {
            session_id: Some(session.to_string()),
            tool_name: Some("Bash".to_string()),
            input_summary: Some(summary.to_string()),
            called_at: Some(at),
            success: Some(true),
            is_sidechain: Some(true),
            agent_id: Some(agent_id.to_string()),
            agent_name: Some(agent_name.to_string()),
            ..TranscriptFacts::default()
        },
    )
    .expect("transcript");
}

fn ids(rows: &[query::TimelineRow]) -> Vec<&str> {
    rows.iter().map(|r| r.call.tool_use_id.as_str()).collect()
}

// ---------------------------------------------------------------------------
// The exit criterion, clause by clause.
// ---------------------------------------------------------------------------

#[test]
fn find_every_rm_rf_then_narrow_to_one_project() {
    let db = seeded();

    let everywhere = TimelineFilter {
        query: Some("rm -rf".to_string()),
        ..TimelineFilter::default()
    };
    let rows = query::timeline_rows(db.conn(), &everywhere, Page::default()).expect("search");
    assert_eq!(
        ids(&rows),
        ["t7", "t6", "t5", "t1"],
        "every rm -rf, newest first — and `-rf` is FTS5 syntax, not a term"
    );
    assert_eq!(
        query::timeline_count(db.conn(), &everywhere).expect("count"),
        4,
        "the count must agree with the page, or the scrollbar lies"
    );

    // Narrowing is the same query with one more clause: search and filter
    // compose, rather than being two lists that disagree.
    let in_app = TimelineFilter {
        project_path: Some("/work/app".to_string()),
        ..everywhere
    };
    let rows = query::timeline_rows(db.conn(), &in_app, Page::default()).expect("search");
    assert_eq!(ids(&rows), ["t6", "t5", "t1"]);
    assert_eq!(rows[0].project_path.as_deref(), Some("/work/app"));
    assert_eq!(rows[0].git_branch.as_deref(), Some("fix/login"));
}

#[test]
fn a_searched_row_says_where_the_match_was() {
    let db = seeded();
    let filter = TimelineFilter {
        query: Some("terraform".to_string()),
        ..TimelineFilter::default()
    };
    let rows = query::timeline_rows(db.conn(), &filter, Page::default()).expect("search");
    let snippet = rows[0]
        .snippet
        .as_deref()
        .expect("a searched row is marked");
    assert!(
        snippet.contains(query::MATCH_OPEN) && snippet.contains(query::MATCH_CLOSE),
        "the match must be delimited for the row to highlight it: {snippet:?}"
    );

    // An unsearched page has nothing to highlight, and says so with a null
    // rather than an empty string the frontend would have to guess about.
    let rows =
        query::timeline_rows(db.conn(), &TimelineFilter::default(), Page::default()).expect("page");
    assert!(rows.iter().all(|r| r.snippet.is_none()));
}

#[test]
fn a_blank_search_term_means_no_constraint_not_no_rows() {
    let db = seeded();
    for blank in ["", "   ", "\t\n"] {
        let filter = TimelineFilter {
            query: Some(blank.to_string()),
            ..TimelineFilter::default()
        };
        assert_eq!(
            query::timeline_count(db.conn(), &filter).expect("count"),
            7,
            "an empty search box shows the timeline, not an empty list"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters (5.6)
// ---------------------------------------------------------------------------

#[test]
fn every_filter_control_narrows_what_it_says_it_does() {
    let db = seeded();
    let count = |f: TimelineFilter| query::timeline_count(db.conn(), &f).expect("count");

    assert_eq!(count(TimelineFilter::default()), 7);
    assert_eq!(
        count(TimelineFilter {
            tool_name: Some("Edit".to_string()),
            ..TimelineFilter::default()
        }),
        1
    );
    assert_eq!(
        count(TimelineFilter {
            success: Some(false),
            ..TimelineFilter::default()
        }),
        1
    );
    assert_eq!(
        count(TimelineFilter {
            since: Some(2_500),
            until: Some(4_000),
            ..TimelineFilter::default()
        }),
        4
    );
    assert_eq!(
        count(TimelineFilter {
            decision_source: Some("user_temporary".to_string()),
            ..TimelineFilter::default()
        }),
        1
    );
    // The mode is a transcript fact, so every main-thread call carries it and
    // the subagent calls — seeded without one — do not.
    assert_eq!(
        count(TimelineFilter {
            permission_mode: Some("default".to_string()),
            ..TimelineFilter::default()
        }),
        5
    );
    assert_eq!(
        count(TimelineFilter {
            session_id: Some("s-infra".to_string()),
            ..TimelineFilter::default()
        }),
        1
    );
}

#[test]
fn the_main_thread_is_partitioned_by_agent_id_not_by_is_sidechain() {
    let db = seeded();

    // `is_sidechain` is null on any row the transcript lane never witnessed, so
    // the reliable partition is the presence of an agent_id.
    project::upsert_otel(
        db.conn(),
        "t-otel-only",
        &OtelFacts {
            session_id: Some("s-app".to_string()),
            tool_name: Some("Bash".to_string()),
            called_at: Some(6_000),
            ..OtelFacts::default()
        },
    )
    .expect("otel");

    let main = TimelineFilter {
        main_thread: Some(true),
        ..TimelineFilter::default()
    };
    let side = TimelineFilter {
        main_thread: Some(false),
        ..TimelineFilter::default()
    };
    assert_eq!(query::timeline_count(db.conn(), &main).expect("count"), 6);
    assert_eq!(query::timeline_count(db.conn(), &side).expect("count"), 2);

    let by_instance = TimelineFilter {
        agent_id: Some("a-1".to_string()),
        ..TimelineFilter::default()
    };
    let rows = query::timeline_rows(db.conn(), &by_instance, Page::default()).expect("rows");
    assert_eq!(ids(&rows), ["t5", "t4"]);
}

// ---------------------------------------------------------------------------
// Filter controls (5.6) and the detail pane (5.8, 5.11)
// ---------------------------------------------------------------------------

#[test]
fn facets_offer_only_values_the_store_actually_holds() {
    let db = seeded();
    let f = query::facets(db.conn()).expect("facets");

    assert_eq!(f.projects, ["/work/app", "/work/infra"]);
    assert_eq!(f.tools, ["Bash", "Edit"], "most-used tool first");
    assert_eq!(f.decision_sources, ["config", "user_temporary"]);
    assert_eq!(f.permission_modes, ["default"]);
    assert_eq!(f.agents, ["Explore"]);
}

#[test]
fn a_call_can_be_traced_back_to_the_transcript_line_that_recorded_it() {
    let db = seeded();
    let body = r#"{"uuid":"u-9","toolUseID":"t9","type":"assistant"}"#;
    toolog_core::raw::insert(
        db.conn(),
        &toolog_core::model::NewRawEvent {
            lane: toolog_core::model::Lane::Transcript,
            source_ref: "/t/s.jsonl",
            source_offset: Some(4_096),
            body,
        },
    )
    .expect("raw");
    project::upsert_transcript(
        db.conn(),
        "t9",
        &TranscriptFacts {
            message_uuid: Some("u-9".to_string()),
            called_at: Some(9_000),
            ..TranscriptFacts::default()
        },
    )
    .expect("transcript");

    let call = query::tool_call_detail(db.conn(), "t9")
        .expect("detail")
        .expect("the call");
    let source = query::source_record(db.conn(), &call)
        .expect("lookup")
        .expect("a transcript line");
    assert_eq!(source.source_ref, "/t/s.jsonl");
    assert_eq!(source.source_offset, Some(4_096));
    assert_eq!(source.body, body, "the stored line is the evidence");
}

#[test]
fn a_call_with_no_stored_line_reports_nothing_rather_than_guessing() {
    let db = seeded();
    let call = query::tool_call_detail(db.conn(), "t1")
        .expect("detail")
        .expect("the call");
    assert!(
        query::source_record(db.conn(), &call)
            .expect("lookup")
            .is_none()
    );
}

#[test]
fn the_session_behind_a_call_carries_the_envelope_the_detail_pane_shows() {
    let db = seeded();
    let s = query::session(db.conn(), "s-app-2")
        .expect("session")
        .expect("found");
    assert_eq!(s.git_branch.as_deref(), Some("fix/login"));
    assert_eq!(s.cc_version.as_deref(), Some("2.1.260"));
    assert!(
        query::session(db.conn(), "nope")
            .expect("session")
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// Paging (5.3)
// ---------------------------------------------------------------------------

#[test]
fn paging_walks_the_whole_result_without_repeating_or_skipping_a_row() {
    let db = seeded();
    let mut seen = Vec::new();
    for offset in (0..8).step_by(2) {
        let page = Page { limit: 2, offset };
        let rows = query::timeline_rows(db.conn(), &TimelineFilter::default(), page).expect("page");
        seen.extend(rows.into_iter().map(|r| r.call.tool_use_id));
    }
    assert_eq!(seen, ["t7", "t6", "t3", "t5", "t4", "t2", "t1"]);
}

// ---------------------------------------------------------------------------
// The activity histogram (tasks 10.1 and 10.2)
// ---------------------------------------------------------------------------

/// A store whose calls are spread over a chosen span, one per step.
fn spread(step_ms: i64, n: i64) -> Db {
    let db = Db::open_in_memory().expect("open");
    let conn = db.conn();
    project::upsert_session(
        conn,
        &Session {
            session_id: "s".to_string(),
            project_path: Some("/work/app".to_string()),
            ..Session::default()
        },
    )
    .expect("session");
    for i in 0..n {
        call(conn, &format!("h{i}"), "s", "Bash", "ls", i * step_ms, true);
    }
    db
}

#[test]
fn the_bucket_size_comes_from_the_span_rather_than_from_the_reader() {
    use query::BucketSize;

    // Chosen so each span lands inside one size's sixty columns and outside
    // the one below it.
    assert_eq!(BucketSize::for_span(0), BucketSize::Minute);
    assert_eq!(BucketSize::for_span(30 * 60_000), BucketSize::Minute);
    assert_eq!(BucketSize::for_span(90 * 60_000), BucketSize::Hour);
    assert_eq!(BucketSize::for_span(20 * 3_600_000), BucketSize::Hour);
    assert_eq!(BucketSize::for_span(5 * 86_400_000), BucketSize::Day);
    assert_eq!(BucketSize::for_span(400 * 86_400_000), BucketSize::Week);

    // Sixty columns is the aim, and the size chosen never spreads a span
    // thinner than it has to be.
    for span in [1, 60_000, 3_600_000, 86_400_000, 400 * 86_400_000] {
        let size = BucketSize::for_span(span);
        assert!(
            span / size.ms() <= 60,
            "a span of {span} ms in {size:?} columns is more than sixty of them"
        );
    }
}

#[test]
fn a_histogram_carries_every_column_in_the_span_including_the_empty_ones() {
    // Three calls an hour apart: hour columns, and the middle hour is empty.
    let db = Db::open_in_memory().expect("open");
    let conn = db.conn();
    project::upsert_session(
        conn,
        &Session {
            session_id: "s".to_string(),
            ..Session::default()
        },
    )
    .expect("session");
    call(conn, "a", "s", "Bash", "ls", 0, true);
    call(conn, "b", "s", "Bash", "ls", 2 * 3_600_000, true);

    let h = query::histogram(conn, &TimelineFilter::default(), 0).expect("histogram");
    assert_eq!(h.size, query::BucketSize::Hour);
    assert_eq!(
        h.buckets.len(),
        3,
        "the empty hour between them is a column"
    );
    assert_eq!(h.buckets[1].calls, 0, "nothing ran, which is not no data");
    assert_eq!(h.buckets[0].calls, 1);
    assert_eq!(h.buckets[2].calls, 1);
}

#[test]
fn a_histogram_and_the_list_can_never_describe_different_rows() {
    let db = spread(86_400_000, 10);
    let conn = db.conn();

    for filter in [
        TimelineFilter::default(),
        TimelineFilter {
            tool_name: Some("Bash".to_string()),
            ..TimelineFilter::default()
        },
        TimelineFilter {
            project_path: Some("/work/app".to_string()),
            ..TimelineFilter::default()
        },
        TimelineFilter {
            query: Some("ls".to_string()),
            ..TimelineFilter::default()
        },
        TimelineFilter {
            since: Some(2 * 86_400_000),
            until: Some(5 * 86_400_000),
            ..TimelineFilter::default()
        },
    ] {
        let counted = query::timeline_count(conn, &filter).expect("count");
        let plotted: i64 = query::histogram(conn, &filter, 0)
            .expect("histogram")
            .buckets
            .iter()
            .map(|b| b.calls)
            .sum();
        assert_eq!(
            counted, plotted,
            "the chart and the count disagree for {filter:?}"
        );
    }
}

#[test]
fn failures_and_refusals_ride_along_rather_than_becoming_a_second_series() {
    let db = seeded();
    let conn = db.conn();
    let h = query::histogram(conn, &TimelineFilter::default(), 0).expect("histogram");

    let calls: i64 = h.buckets.iter().map(|b| b.calls).sum();
    let failures: i64 = h.buckets.iter().map(|b| b.failures).sum();
    let refusals: i64 = h.buckets.iter().map(|b| b.refusals).sum();
    assert_eq!(calls, 7, "every call in the store");
    assert_eq!(failures, 1, "t6 is the failure");
    assert_eq!(refusals, 0, "nothing in this fixture was refused");
    assert!(
        failures <= calls && refusals <= calls,
        "the carried counts are a slice of the measure, not a second one"
    );
}

#[test]
fn days_are_the_readers_days() {
    // 23:30 UTC. In UTC that is one day; two hours east it is already the next.
    let db = Db::open_in_memory().expect("open");
    let conn = db.conn();
    project::upsert_session(
        conn,
        &Session {
            session_id: "s".to_string(),
            ..Session::default()
        },
    )
    .expect("session");
    let late = 23 * 3_600_000 + 30 * 60_000;
    call(conn, "a", "s", "Bash", "ls", 0, true);
    call(conn, "b", "s", "Bash", "ls", late + 6 * 86_400_000, true);

    let utc = query::histogram(conn, &TimelineFilter::default(), 0).expect("histogram");
    let east = query::histogram(conn, &TimelineFilter::default(), 120).expect("histogram");
    assert_eq!(utc.size, query::BucketSize::Day);

    // The same two calls, and a column boundary that moved with the reader.
    assert_eq!(utc.buckets[0].start_ms, 0, "local midnight at UTC is 0");
    assert_eq!(
        east.buckets[0].start_ms,
        -2 * 3_600_000,
        "two hours east, the day that contains the epoch started before it"
    );
    assert_eq!(
        utc.buckets.len() + 1,
        east.buckets.len(),
        "the late call falls into tomorrow once the reader is east of Greenwich"
    );
}

#[test]
fn a_column_start_is_the_instant_a_brush_can_write_into_the_hash() {
    let db = spread(3_600_000, 5);
    let conn = db.conn();
    let h = query::histogram(conn, &TimelineFilter::default(), 0).expect("histogram");
    let width = h.size.ms();

    // Narrowing to one column's own range yields exactly that column's calls,
    // which is what makes click-to-filter honest (task 10.4).
    for bucket in &h.buckets {
        let narrowed = TimelineFilter {
            since: Some(bucket.start_ms),
            until: Some(bucket.start_ms + width - 1),
            ..TimelineFilter::default()
        };
        assert_eq!(
            query::timeline_count(conn, &narrowed).expect("count"),
            bucket.calls,
            "the column starting at {} does not reproduce itself",
            bucket.start_ms
        );
    }
}

#[test]
fn an_empty_store_has_no_columns_rather_than_a_grid_of_zeroes() {
    let db = Db::open_in_memory().expect("open");
    let h = query::histogram(db.conn(), &TimelineFilter::default(), 0).expect("histogram");
    assert!(h.buckets.is_empty());
    assert_eq!(h.since_ms, None);
    assert_eq!(h.until_ms, None);
}
