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
// Session grouping (5.10)
// ---------------------------------------------------------------------------

#[test]
fn groups_carry_the_sizes_a_collapsible_list_needs() {
    let db = seeded();
    let groups = query::timeline_groups(db.conn(), &TimelineFilter::default()).expect("groups");

    let keys: Vec<_> = groups
        .iter()
        .map(|g| g.session_id.as_deref().unwrap_or("?"))
        .collect();
    assert_eq!(
        keys,
        ["s-infra", "s-app-2", "s-app"],
        "sessions order by their most recent call, like the rows do"
    );

    let app = &groups[2];
    assert_eq!(app.calls, 5);
    assert_eq!(
        app.main_thread_calls, 3,
        "a collapsed session's height is known without fetching a row"
    );
    assert_eq!(app.project_path.as_deref(), Some("/work/app"));
    assert_eq!(app.first_at, Some(1_000));
    assert_eq!(app.last_at, Some(3_000));

    assert_eq!(app.agents.len(), 1, "subagents are groups, not stray rows");
    assert_eq!(app.agents[0].agent_id, "a-1");
    assert_eq!(app.agents[0].agent_name.as_deref(), Some("Explore"));
    assert_eq!(app.agents[0].calls, 2);
    assert_eq!(
        app.calls - app.main_thread_calls,
        app.agents.iter().map(|a| a.calls).sum::<i64>(),
        "every call is in exactly one group, or the list has gaps or repeats"
    );

    assert_eq!(groups[1].failures, 1);
}

#[test]
fn groups_narrow_with_the_filter_but_cost_does_not() {
    let db = seeded();
    project::upsert_api_request(
        db.conn(),
        &toolog_core::model::ApiRequest {
            request_id: "r1".to_string(),
            session_id: Some("s-app".to_string()),
            cost_usd_micros: Some(4_200),
            ..toolog_core::model::ApiRequest::default()
        },
    )
    .expect("api request");

    let filter = TimelineFilter {
        tool_name: Some("Edit".to_string()),
        ..TimelineFilter::default()
    };
    let groups = query::timeline_groups(db.conn(), &filter).expect("groups");
    assert_eq!(groups.len(), 1, "only sessions the filter still touches");
    assert_eq!(groups[0].calls, 1);
    assert_eq!(
        groups[0].cost_usd_micros,
        Some(4_200),
        "narrowing the view must not make a session look cheaper than it was"
    );

    let groups = query::timeline_groups(db.conn(), &TimelineFilter::default()).expect("groups");
    assert!(
        groups
            .iter()
            .filter(|g| g.session_id.as_deref() != Some("s-app"))
            .all(|g| g.cost_usd_micros.is_none()),
        "a session OTEL never saw has no cost — not a cost of zero, which reads as free"
    );
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
