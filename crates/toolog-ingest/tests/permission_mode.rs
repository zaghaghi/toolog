//! The permission mode, which the plan had coming from the wrong lane.
//!
//! ADR-0002 and ADR-0009 both list `permission_mode` among the columns only the
//! OTLP lane can supply. Measured against Claude Code 2.1.260, it supplies none
//! of it: 987 OTLP records across 11 event types carry no `permission_mode`
//! attribute and no `permission_mode_changed` event. The transcript carries it,
//! in two shapes, and this pins the behaviour built on that.

use std::io::Write as _;

use toolog_core::model::{Page, TimelineFilter};
use toolog_core::{Connection, Db, query};
use toolog_ingest::Backfill;

/// Write `lines` as one transcript and ingest it.
fn ingest(lines: &[&str]) -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut file = std::fs::File::create(&path).expect("create");
    for line in lines {
        // These fixtures are wrapped for readability; JSONL is one record per
        // line, and a stray newline would make every one of them unparsable.
        writeln!(file, "{}", line.replace('\n', " ")).expect("write");
    }
    drop(file);

    let db = Db::open_in_memory().expect("open");
    Backfill::new(db.conn()).run(dir.path()).expect("backfill");
    (dir, db)
}

fn mode_of(conn: &Connection, tool_use_id: &str) -> Option<String> {
    query::tool_call_detail(conn, tool_use_id)
        .expect("detail")
        .expect("the call")
        .permission_mode
}

fn changes(conn: &Connection) -> Vec<(Option<String>, Option<String>, Option<String>)> {
    let mut stmt = conn
        .prepare("SELECT from_mode, to_mode, trigger FROM permission_mode_change ORDER BY id")
        .expect("prepare");
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query");
    rows.collect::<rusqlite::Result<Vec<_>>>().expect("rows")
}

/// An assistant record making one tool call.
fn call(uuid: &str, id: &str, at: &str) -> String {
    format!(
        r#"{{"type":"assistant","uuid":"{uuid}","sessionId":"s1","timestamp":"{at}",
            "cwd":"/work","message":{{"role":"assistant","content":[
              {{"type":"tool_use","id":"{id}","name":"Bash","input":{{"command":"ls"}}}}]}}}}"#
    )
    .replace('\n', "")
}

#[test]
fn a_permission_mode_record_sets_the_mode_for_the_calls_that_follow() {
    let (_dir, db) = ingest(&[
        r#"{"type":"permission-mode","permissionMode":"auto","sessionId":"s1"}"#,
        &call("u1", "toolu_1", "2026-09-05T09:00:00.000Z"),
        r#"{"type":"permission-mode","permissionMode":"dontAsk","sessionId":"s1"}"#,
        &call("u2", "toolu_2", "2026-09-05T09:01:00.000Z"),
    ]);

    assert_eq!(mode_of(db.conn(), "toolu_1").as_deref(), Some("auto"));
    assert_eq!(
        mode_of(db.conn(), "toolu_2").as_deref(),
        Some("dontAsk"),
        "the mode holds until another record changes it"
    );
}

#[test]
fn a_user_turn_carries_the_mode_too_and_resynchronises_it() {
    let (_dir, db) = ingest(&[
        r#"{"type":"permission-mode","permissionMode":"plan","sessionId":"s1"}"#,
        &call("u1", "toolu_1", "2026-09-05T09:00:00.000Z"),
        r#"{"type":"user","uuid":"u2","sessionId":"s1","permissionMode":"default",
            "timestamp":"2026-09-05T09:01:00.000Z","cwd":"/work",
            "message":{"role":"user","content":"hi"}}"#,
        &call("u3", "toolu_2", "2026-09-05T09:02:00.000Z"),
    ]);

    assert_eq!(mode_of(db.conn(), "toolu_1").as_deref(), Some("plan"));
    assert_eq!(mode_of(db.conn(), "toolu_2").as_deref(), Some("default"));
}

#[test]
fn only_the_changes_are_recorded_not_every_record_that_repeats_the_mode() {
    let (_dir, db) = ingest(&[
        r#"{"type":"permission-mode","permissionMode":"auto","sessionId":"s1"}"#,
        &call("u1", "toolu_1", "2026-09-05T09:00:00.000Z"),
        r#"{"type":"user","uuid":"u2","sessionId":"s1","permissionMode":"auto",
            "timestamp":"2026-09-05T09:01:00.000Z","cwd":"/work",
            "message":{"role":"user","content":"still auto"}}"#,
        r#"{"type":"permission-mode","permissionMode":"dontAsk","sessionId":"s1"}"#,
        &call("u3", "toolu_2", "2026-09-05T09:02:00.000Z"),
    ]);

    assert_eq!(
        changes(db.conn()),
        vec![
            // The first observation is how the session started, and is recorded
            // with no `from_mode` rather than skipped.
            (None, Some("auto".into()), Some("permission-mode".into())),
            (
                Some("auto".into()),
                Some("dontAsk".into()),
                Some("permission-mode".into())
            ),
        ]
    );
}

#[test]
fn a_permission_mode_record_is_a_known_record_type() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("s.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"permission-mode\",\"permissionMode\":\"auto\",\"sessionId\":\"s1\"}\n",
    )
    .expect("write");

    let db = Db::open_in_memory().expect("open");
    let report = Backfill::new(db.conn()).run(dir.path()).expect("backfill");
    assert_eq!(
        report.stats.unknown_records, 0,
        "a record type we act on must not also be counted as unrecognised"
    );
}

#[test]
fn the_mode_survives_a_re_projection_from_evidence() {
    let (_dir, db) = ingest(&[
        r#"{"type":"permission-mode","permissionMode":"dontAsk","sessionId":"s1"}"#,
        &call("u1", "toolu_1", "2026-09-05T09:00:00.000Z"),
    ]);
    assert_eq!(mode_of(db.conn(), "toolu_1").as_deref(), Some("dontAsk"));

    // One lane's projector is enough here because this store holds one lane.
    // The all-lanes rebuild is `toolog backfill --reproject`.
    let mut projector = toolog_ingest::TranscriptProjector::new();
    toolog_core::project::reproject(db.conn(), None, &mut projector).expect("reproject");

    assert_eq!(
        mode_of(db.conn(), "toolu_1").as_deref(),
        Some("dontAsk"),
        "the mode is derived from stored records, so rebuilding must reproduce it"
    );
    assert_eq!(
        changes(db.conn()).len(),
        1,
        "and not double-count the change"
    );
}

/// A subagent's calls arrive in their own transcript, which carries no
/// `permission-mode` record of its own.
#[test]
fn a_subagent_inherits_the_mode_of_the_session_it_was_spawned_in() {
    let dir = tempfile::tempdir().expect("tempdir");
    // The subagent's file sorts first, so it is projected before the parent's —
    // the order that leaves nothing in hand when its calls go by.
    std::fs::write(
        dir.path().join("a-subagent.jsonl"),
        format!(
            "{}\n",
            r#"{"type":"assistant","uuid":"s1u","sessionId":"s1","isSidechain":true,
               "agentId":"ag-1","timestamp":"2026-09-05T09:05:00.000Z","cwd":"/work",
               "message":{"role":"assistant","content":[
                 {"type":"tool_use","id":"toolu_sub","name":"Bash","input":{"command":"ls"}}]}}"#
                .replace('\n', " ")
        ),
    )
    .expect("write");
    std::fs::write(
        dir.path().join("b-parent.jsonl"),
        format!(
            "{}\n{}\n",
            r#"{"type":"user","uuid":"u1","sessionId":"s1","permissionMode":"dontAsk",
               "timestamp":"2026-09-05T09:00:00.000Z","cwd":"/work",
               "message":{"role":"user","content":"go"}}"#
                .replace('\n', " "),
            call("u2", "toolu_main", "2026-09-05T09:06:00.000Z"),
        ),
    )
    .expect("write");

    let db = Db::open_in_memory().expect("open");
    Backfill::new(db.conn()).run(dir.path()).expect("backfill");

    assert_eq!(mode_of(db.conn(), "toolu_main").as_deref(), Some("dontAsk"));
    assert_eq!(
        mode_of(db.conn(), "toolu_sub").as_deref(),
        Some("dontAsk"),
        "a subagent runs under its session's mode, whichever file arrives first"
    );
}

#[test]
fn the_mode_is_filterable_from_the_timeline() {
    let (_dir, db) = ingest(&[
        r#"{"type":"permission-mode","permissionMode":"dontAsk","sessionId":"s1"}"#,
        &call("u1", "toolu_1", "2026-09-05T09:00:00.000Z"),
        r#"{"type":"permission-mode","permissionMode":"default","sessionId":"s1"}"#,
        &call("u2", "toolu_2", "2026-09-05T09:01:00.000Z"),
    ]);

    let risky = TimelineFilter {
        permission_mode: Some("dontAsk".to_string()),
        ..TimelineFilter::default()
    };
    let rows = query::timeline_rows(db.conn(), &risky, Page::default()).expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].call.tool_use_id, "toolu_1");

    let facets = query::facets(db.conn()).expect("facets");
    assert_eq!(facets.permission_modes, ["default", "dontAsk"]);
}
