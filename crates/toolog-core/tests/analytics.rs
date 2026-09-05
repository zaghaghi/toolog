//! Usage analytics against a store with a deliberate hole in it (task 6.5).
//!
//! The hole is the point. Two of the three sessions here were captured live and
//! have cost; one was backfilled and never will. Every test that touches money
//! also checks what the store says about how much of the picture it has, because
//! task 6.8 turns on that answer being available rather than inferred.

use toolog_core::analytics::{self, Period};
use toolog_core::db::Db;
use toolog_core::model::{ApiRequest, OtelFacts, Session, TranscriptFacts};
use toolog_core::{Connection, project};

/// 2026-03-02T00:00:00Z, a Monday, so day bucketing has a fixed reference.
const MONDAY: i64 = 1_772_409_600_000;
const HOUR: i64 = 3_600_000;
const DAY: i64 = 86_400_000;

fn session(conn: &Connection, id: &str, project_path: &str) {
    project::upsert_session(
        conn,
        &Session {
            session_id: id.to_string(),
            project_path: Some(project_path.to_string()),
            cwd: Some(project_path.to_string()),
            ..Session::default()
        },
    )
    .expect("session");
}

struct Call<'a> {
    id: &'a str,
    session_id: &'a str,
    tool: &'a str,
    at: i64,
    success: Option<bool>,
    agent_id: Option<&'a str>,
}

fn call(conn: &Connection, c: &Call<'_>) {
    project::upsert_transcript(
        conn,
        c.id,
        &TranscriptFacts {
            session_id: Some(c.session_id.to_string()),
            tool_name: Some(c.tool.to_string()),
            tool_kind: Some("builtin".to_string()),
            called_at: Some(c.at),
            success: c.success,
            agent_id: c.agent_id.map(ToString::to_string),
            is_sidechain: c.agent_id.map(|_| true),
            ..TranscriptFacts::default()
        },
    )
    .expect("call");
}

/// A plain main-thread call that succeeded.
fn ok(conn: &Connection, id: &str, session_id: &str, tool: &str, at: i64) {
    call(
        conn,
        &Call {
            id,
            session_id,
            tool,
            at,
            success: Some(true),
            agent_id: None,
        },
    );
}

fn duration(conn: &Connection, id: &str, ms: i64) {
    project::upsert_otel(
        conn,
        id,
        &OtelFacts {
            duration_ms: Some(ms),
            ..OtelFacts::default()
        },
    )
    .expect("duration");
}

struct Spend<'a> {
    id: &'a str,
    session_id: &'a str,
    model: &'a str,
    at: i64,
    micros: i64,
    input: i64,
    output: i64,
    cached: i64,
}

fn spend(conn: &Connection, s: &Spend<'_>) {
    project::upsert_api_request(
        conn,
        &ApiRequest {
            request_id: s.id.to_string(),
            session_id: Some(s.session_id.to_string()),
            model: Some(s.model.to_string()),
            cost_usd_micros: Some(s.micros),
            input_tokens: Some(s.input),
            output_tokens: Some(s.output),
            cache_read_tokens: Some(s.cached),
            cache_creation_tokens: Some(0),
            ts: Some(s.at),
            ..ApiRequest::default()
        },
    )
    .expect("api request");
}

/// Three sessions: two priced, one backfilled and costless.
fn store() -> Db {
    let db = Db::open_in_memory().expect("open");
    let conn = db.conn();

    session(conn, "live-1", "/work/alpha");
    session(conn, "live-2", "/work/beta");
    session(conn, "old-1", "/work/alpha");

    // Monday, /work/alpha, captured live. Four calls one minute apart, one of
    // which failed, plus one subagent call.
    for (i, tool) in ["Bash", "Read", "Edit", "Bash"].iter().enumerate() {
        #[expect(clippy::cast_possible_wrap, reason = "four fixture rows")]
        let at = MONDAY + 9 * HOUR + i as i64 * 60_000;
        ok(conn, &format!("live-1-{i}"), "live-1", tool, at);
    }
    project::upsert_transcript(
        conn,
        "live-1-3",
        &TranscriptFacts {
            success: Some(false),
            ..TranscriptFacts::default()
        },
    )
    .expect("failure");
    call(
        conn,
        &Call {
            id: "live-1-sub",
            session_id: "live-1",
            tool: "Grep",
            at: MONDAY + 9 * HOUR + 240_000,
            success: Some(true),
            agent_id: Some("ag-1"),
        },
    );
    duration(conn, "live-1-0", 100);
    duration(conn, "live-1-1", 200);
    duration(conn, "live-1-2", 300);
    duration(conn, "live-1-3", 4_000);
    spend(
        conn,
        &Spend {
            id: "req-1",
            session_id: "live-1",
            model: "claude-opus-5",
            at: MONDAY + 9 * HOUR,
            micros: 1_500_000,
            input: 1_000,
            output: 500,
            cached: 9_000,
        },
    );

    // Tuesday, /work/beta, also captured live.
    ok(conn, "live-2-0", "live-2", "Bash", MONDAY + DAY + 10 * HOUR);
    ok(
        conn,
        "live-2-1",
        "live-2",
        "Write",
        MONDAY + DAY + 10 * HOUR + 30_000,
    );
    spend(
        conn,
        &Spend {
            id: "req-2",
            session_id: "live-2",
            model: "claude-sonnet-5",
            at: MONDAY + DAY + 10 * HOUR,
            micros: 250_000,
            input: 2_000,
            output: 300,
            cached: 0,
        },
    );

    // Wednesday, /work/alpha, backfilled: calls but no cost, ever.
    ok(
        conn,
        "old-1-0",
        "old-1",
        "Bash",
        MONDAY + 2 * DAY + 11 * HOUR,
    );
    ok(
        conn,
        "old-1-1",
        "old-1",
        "Bash",
        // An hour later: past the idle cutoff, so not active time.
        MONDAY + 2 * DAY + 12 * HOUR,
    );

    db
}

/// The whole corpus, bucketed in UTC.
fn everything() -> Period {
    Period::default()
}

#[test]
fn call_counts_cover_the_whole_corpus_and_cost_does_not() {
    let db = store();
    let a = analytics::analytics(db.conn(), &everything()).expect("analytics");

    assert_eq!(a.calls.calls, 9, "every call, priced or not");
    assert_eq!(a.calls.sessions, 3);
    assert_eq!(a.calls.projects, 2);
    assert_eq!(a.cost.cost_usd_micros, 1_750_000);

    assert_eq!(a.coverage.sessions, 3);
    assert_eq!(
        a.coverage.sessions_with_cost, 2,
        "the backfilled session has no cost and the UI must be able to say so"
    );
    assert_eq!(a.coverage.calls, 9);
    assert_eq!(a.coverage.calls_with_cost, 7);
    assert!(a.coverage.measured && !a.coverage.complete);
}

#[test]
fn an_error_rate_divides_by_the_calls_with_a_known_outcome() {
    let db = store();
    let conn = db.conn();
    // A call witnessed only by OTEL has no success column at all, and counting
    // it as a success would flatter the rate.
    project::upsert_otel(
        conn,
        "refused-1",
        &OtelFacts {
            session_id: Some("live-1".to_string()),
            decision: Some("reject".to_string()),
            called_at: Some(MONDAY + 9 * HOUR),
            ..OtelFacts::default()
        },
    )
    .expect("refusal");

    let a = analytics::analytics(conn, &everything()).expect("analytics");
    assert_eq!(a.calls.calls, 10);
    assert_eq!(a.calls.with_outcome, 9);
    assert_eq!(a.calls.failures, 1);
    assert_eq!(a.calls.refused, 1);
    let rate = a.calls.error_rate.expect("some outcomes");
    assert!((rate - 1.0 / 9.0).abs() < f64::EPSILON, "{rate}");
}

#[test]
fn sidechain_share_and_cache_hit_ratio_are_fractions_of_what_was_measured() {
    let db = store();
    let a = analytics::analytics(db.conn(), &everything()).expect("analytics");

    assert_eq!(a.calls.sidechain, 1);
    let share = a.calls.sidechain_share.expect("calls");
    assert!((share - 1.0 / 9.0).abs() < f64::EPSILON, "{share}");

    // 9,000 cached of 12,000 input across both priced requests.
    let ratio = a.cost.cache_hit_ratio.expect("tokens");
    assert!((ratio - 0.75).abs() < f64::EPSILON, "{ratio}");
    assert_eq!(a.cost.total_tokens, 12_800);
}

#[test]
fn percentiles_come_from_the_lane_that_times_calls() {
    let db = store();
    let a = analytics::analytics(db.conn(), &everything()).expect("analytics");
    // Four timed calls: 100, 200, 300, 4000.
    assert_eq!(a.calls.p50_ms, Some(200));
    assert_eq!(a.calls.p95_ms, Some(4_000));

    let bash = a
        .tools
        .iter()
        .find(|t| t.tool_name == "Bash")
        .expect("Bash");
    assert_eq!(bash.calls, 5, "three live, two backfilled");
    assert_eq!(bash.failures, 1);
    assert_eq!(
        bash.p50_ms,
        Some(100),
        "only the calls OTEL timed have a duration"
    );
}

#[test]
fn active_time_stops_at_the_idle_cutoff() {
    let db = store();
    let a = analytics::analytics(db.conn(), &everything()).expect("analytics");

    // live-1: four one-minute gaps. live-2: one 30-second gap. old-1's hour is
    // past the cutoff and contributes nothing — the store cannot see whether
    // anyone was there, and counting it would invent an hour of work.
    assert_eq!(a.calls.active_ms, 4 * 60_000 + 30_000);
}

#[test]
fn days_are_bucketed_in_the_readers_timezone() {
    let db = store();
    let utc = analytics::analytics(db.conn(), &everything()).expect("analytics");
    assert_eq!(
        utc.by_day
            .iter()
            .map(|b| b.key.as_deref())
            .collect::<Vec<_>>(),
        [Some("2026-03-02"), Some("2026-03-03"), Some("2026-03-04")]
    );
    assert_eq!(utc.by_day[0].calls, 5);
    assert_eq!(utc.by_day[0].cost_usd_micros, 1_500_000);

    // Sixteen hours west: Monday 09:00Z is Sunday evening, and the reader's
    // Sunday is where that work belongs.
    let west = Period {
        utc_offset_minutes: -16 * 60,
        ..Period::default()
    };
    let shifted = analytics::analytics(db.conn(), &west).expect("analytics");
    assert_eq!(shifted.by_day[0].key.as_deref(), Some("2026-03-01"));
}

#[test]
fn the_project_leaderboard_ranks_by_cost_then_by_calls() {
    let db = store();
    let a = analytics::analytics(db.conn(), &everything()).expect("analytics");

    let names: Vec<_> = a.by_project.iter().map(|b| b.key.as_deref()).collect();
    assert_eq!(names, [Some("/work/alpha"), Some("/work/beta")]);

    let alpha = &a.by_project[0];
    assert_eq!(alpha.calls, 7, "both alpha sessions");
    assert_eq!(alpha.cost_usd_micros, 1_500_000, "only the priced one");
    assert_eq!(alpha.failures, 1);
}

#[test]
fn a_model_breakdown_counts_requests_because_calls_have_no_model() {
    let db = store();
    let a = analytics::analytics(db.conn(), &everything()).expect("analytics");

    assert_eq!(
        a.by_model
            .iter()
            .map(|b| (b.key.as_deref(), b.requests, b.calls))
            .collect::<Vec<_>>(),
        [
            (Some("claude-opus-5"), 1, 0),
            (Some("claude-sonnet-5"), 1, 0)
        ]
    );
}

#[test]
fn sessions_are_newest_first_and_carry_the_project_that_names_them() {
    let db = store();
    let a = analytics::analytics(db.conn(), &everything()).expect("analytics");

    let ids: Vec<_> = a.by_session.iter().map(|b| b.key.as_deref()).collect();
    assert_eq!(ids, [Some("old-1"), Some("live-2"), Some("live-1")]);
    assert_eq!(a.by_session[0].label.as_deref(), Some("/work/alpha"));
    assert_eq!(a.by_session[0].cost_usd_micros, 0);
    assert_eq!(a.by_session[2].cost_usd_micros, 1_500_000);
}

#[test]
fn a_window_narrows_calls_and_cost_together() {
    let db = store();
    let tuesday = Period {
        since: Some(MONDAY + DAY),
        until: Some(MONDAY + 2 * DAY),
        ..Period::default()
    };
    let a = analytics::analytics(db.conn(), &tuesday).expect("analytics");

    assert_eq!(a.calls.calls, 2);
    assert_eq!(a.calls.sessions, 1);
    assert_eq!(a.cost.cost_usd_micros, 250_000);
    assert_eq!(a.by_day.len(), 1);
}

#[test]
fn a_project_filter_applies_to_both_tables() {
    let db = store();
    let beta = Period {
        project_path: Some("/work/beta".to_string()),
        ..Period::default()
    };
    let a = analytics::analytics(db.conn(), &beta).expect("analytics");

    assert_eq!(a.calls.calls, 2);
    assert_eq!(a.cost.cost_usd_micros, 250_000);
    assert_eq!(a.by_project.len(), 1);
    assert!(a.coverage.complete, "beta's one session was captured live");
}

#[test]
fn a_window_is_compared_with_the_period_immediately_before_it() {
    let db = store();
    let wednesday = Period {
        since: Some(MONDAY + 2 * DAY),
        until: Some(MONDAY + 3 * DAY),
        ..Period::default()
    };
    let c = analytics::compare(db.conn(), &wednesday).expect("compare");

    assert_eq!(c.current.calls, 2);
    assert_eq!(c.current.cost_usd_micros, 0);
    assert_eq!(
        c.current.sessions_with_cost, 0,
        "spend falling to zero and coverage falling to zero look identical \
         without this"
    );

    let previous = c.previous.expect("Tuesday");
    assert_eq!(previous.calls, 2);
    assert_eq!(previous.cost_usd_micros, 250_000);
    assert_eq!(previous.sessions_with_cost, 1);
    assert_eq!(c.previous_window.expect("window").since, Some(MONDAY + DAY));
}

#[test]
fn an_unbounded_window_has_nothing_to_compare_against() {
    let db = store();
    let c = analytics::compare(db.conn(), &everything()).expect("compare");
    assert_eq!(c.current.calls, 9);
    assert!(c.previous.is_none());
    assert!(c.previous_window.is_none());
}

#[test]
fn an_empty_store_reports_nothing_rather_than_zeroes_that_look_measured() {
    let db = Db::open_in_memory().expect("open");
    let a = analytics::analytics(db.conn(), &everything()).expect("analytics");

    assert_eq!(a.calls.calls, 0);
    assert!(a.calls.error_rate.is_none());
    assert!(a.cost.cache_hit_ratio.is_none());
    assert!(!a.coverage.measured);
    assert!(a.by_day.is_empty() && a.by_project.is_empty() && a.tools.is_empty());
}

// ---------------------------------------------------------------------------
// Live monitoring (tasks 6.10 and 6.11)
// ---------------------------------------------------------------------------

#[test]
fn concurrent_sessions_are_separate_lanes_with_their_own_attribution() {
    let db = store();
    let conn = db.conn();
    // Two sessions with calls inside the window, one outside it.
    let now = MONDAY + 2 * DAY + 12 * HOUR;
    let lanes = analytics::live_sessions(conn, now - 30 * 60_000, now).expect("live");

    assert_eq!(
        lanes
            .iter()
            .map(|l| l.session_id.as_str())
            .collect::<Vec<_>>(),
        ["old-1"],
        "only the session with a call in the window"
    );
    let lane = &lanes[0];
    assert_eq!(lane.project_path.as_deref(), Some("/work/alpha"));
    assert_eq!(lane.current_tool.as_deref(), Some("Bash"));
    assert_eq!(lane.calls, 1, "the calls inside the window");
    assert!(
        !lane.priced,
        "a backfilled session reports no cost, and says which it is"
    );
    assert_eq!(lane.cost_usd_micros, 0);
}

#[test]
fn a_lane_carries_the_cost_of_the_session_it_names() {
    let db = store();
    let now = MONDAY + 9 * HOUR + 300_000;
    let lanes = analytics::live_sessions(db.conn(), now - 30 * 60_000, now).expect("live");

    let live = lanes
        .iter()
        .find(|l| l.session_id == "live-1")
        .expect("live-1");
    assert!(live.priced);
    assert_eq!(live.cost_usd_micros, 1_500_000);
    assert_eq!(live.failures, 1);
    assert_eq!(
        live.current_tool.as_deref(),
        Some("Grep"),
        "the newest call"
    );
}

#[test]
fn a_lane_reports_calls_per_minute_oldest_first() {
    let db = store();
    // Five calls in live-1 land in the same minute, one minute apart, ending
    // four minutes after the first.
    let now = MONDAY + 9 * HOUR + 240_000;
    let lanes = analytics::live_sessions(db.conn(), now - 30 * 60_000, now).expect("live");
    let live = lanes
        .iter()
        .find(|l| l.session_id == "live-1")
        .expect("live-1");

    assert_eq!(live.recent.len(), 12, "twelve one-minute buckets");
    assert_eq!(
        live.recent.iter().sum::<i64>(),
        5,
        "every call in the window"
    );
    assert_eq!(
        live.recent.last(),
        Some(&1),
        "the newest minute is the last bucket, so it reads left to right like time"
    );
    assert_eq!(
        live.recent,
        [0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1],
        "one call a minute for five minutes, and quiet before that"
    );
}

#[test]
fn a_window_with_no_calls_has_no_lanes_rather_than_an_error() {
    let db = store();
    let now = MONDAY + 30 * DAY;
    assert!(
        analytics::live_sessions(db.conn(), now - 60_000, now)
            .expect("live")
            .is_empty()
    );
}
