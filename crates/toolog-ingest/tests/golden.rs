//! Golden tests — the transcript parser's contract.
//!
//! Fixtures live in `fixtures/transcripts/` and are **synthetic**, not scrubbed
//! recordings. The corpus this parser was written against holds 226,192 string
//! leaves totalling 37.3 MB of free text: prompts, source code, client project
//! names. Reliably redacting that for a public repository is not achievable, and
//! one miss publishes someone's code.
//!
//! Instead the fixtures reproduce the *structures* the corpus exhibits, drawn
//! from `fixtures/schema-manifest.json` — key names and value types extracted
//! from real data with every value discarded. Structural fidelity, zero content.
//!
//! `real_corpus.rs` covers what only real data can, without committing any.

use std::path::PathBuf;

use toolog_core::Db;
use toolog_core::model::{Page, TimelineFilter};
use toolog_core::query;
use toolog_ingest::Backfill;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/transcripts")
}

fn ingested() -> Db {
    let db = Db::open_in_memory().expect("open");
    Backfill::new(db.conn()).run(&fixtures()).expect("backfill");
    db
}

#[test]
fn backfill_reads_every_fixture_without_failing() {
    let db = Db::open_in_memory().expect("open");
    let report = Backfill::new(db.conn()).run(&fixtures()).expect("backfill");

    assert_eq!(report.files, 3);
    assert_eq!(report.stored, report.lines, "a first run stores everything");
    assert_eq!(
        report.stats.unparsable, 1,
        "exactly the one deliberately broken line"
    );
    assert!(
        report.stats.tool_uses >= 12,
        "got {}",
        report.stats.tool_uses
    );
    assert!(
        report.stats.unknown_records > 0,
        "unknown types are counted, not fatal"
    );
}

#[test]
fn re_running_a_backfill_changes_nothing() {
    let db = Db::open_in_memory().expect("open");
    let root = fixtures();

    let first = Backfill::new(db.conn()).run(&root).expect("first");
    let before = query::stats_totals(db.conn()).expect("totals");

    let second = Backfill::new(db.conn()).run(&root).expect("second");
    let after = query::stats_totals(db.conn()).expect("totals");

    assert_eq!(second.stored, 0, "everything was already held");
    assert_eq!(
        second.duplicates, second.lines,
        "a resume re-reads only the last stored line per file, and dedups it"
    );
    assert!(
        second.lines < first.lines,
        "resuming skips what was already read"
    );
    assert_eq!(before.raw_events, after.raw_events);
    assert_eq!(before.tool_calls, after.tool_calls);
}

#[test]
fn bash_calls_carry_their_command_and_outcome() {
    let db = ingested();

    let ok = query::tool_call_detail(db.conn(), "toolu_bash_ok")
        .expect("q")
        .expect("row");
    assert_eq!(ok.tool_name.as_deref(), Some("Bash"));
    assert_eq!(ok.tool_kind.as_deref(), Some("builtin"));
    assert_eq!(ok.input_summary.as_deref(), Some("cargo test --workspace"));
    assert_eq!(ok.success, Some(true));
    assert!(
        ok.result_text
            .as_deref()
            .unwrap_or_default()
            .contains("12 passed")
    );

    // A bare-string result is always an error; the tool's is_error confirms it.
    let err = query::tool_call_detail(db.conn(), "toolu_bash_err")
        .expect("q")
        .expect("row");
    assert_eq!(err.success, Some(false));
    assert!(
        err.result_text
            .as_deref()
            .unwrap_or_default()
            .contains("No such file")
    );

    let interrupted = query::tool_call_detail(db.conn(), "toolu_interrupted")
        .expect("q")
        .expect("row");
    assert_eq!(interrupted.success, Some(false), "interrupted is a failure");
}

#[test]
fn edits_produce_file_changes_with_line_counts() {
    let db = ingested();

    let edit = query::tool_call_detail(db.conn(), "toolu_edit")
        .expect("q")
        .expect("row");
    assert_eq!(edit.target_path.as_deref(), Some("/work/app/src/main.rs"));

    let changes = query::file_changes(db.conn(), "toolu_edit").expect("changes");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].file_path, "/work/app/src/main.rs");
    assert_eq!(changes[0].lines_added, 3, "+fn main() {{, +serve();, +}}");
    assert_eq!(changes[0].lines_removed, 1);

    // A Write that creates a file has an empty patch but is still a change.
    let created = query::file_changes(db.conn(), "toolu_write").expect("changes");
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].lines_added, 0);
}

#[test]
fn mcp_tools_are_split_into_server_and_tool() {
    let db = ingested();
    let call = query::tool_call_detail(db.conn(), "toolu_mcp")
        .expect("q")
        .expect("row");

    assert_eq!(call.tool_kind.as_deref(), Some("mcp"));
    assert_eq!(call.mcp_server.as_deref(), Some("preview"));
    assert_eq!(call.mcp_tool.as_deref(), Some("screenshot"));
}

/// A single screenshot was the largest result in the planning corpus at 610 KiB.
#[test]
fn base64_payloads_are_elided_from_the_projection() {
    let db = ingested();
    let call = query::tool_call_detail(db.conn(), "toolu_mcp")
        .expect("q")
        .expect("row");

    let json = call.result_json.as_deref().expect("result stored");
    assert!(
        json.contains("[elided:"),
        "large base64 is described, not inlined"
    );
    assert!(
        !json.contains("QUJDRA0KQUJDRA0KQUJDRA0K"),
        "the payload itself is gone"
    );

    let text = call.result_text.as_deref().expect("text");
    assert!(text.contains("Captured viewport."), "real text survives");
    assert!(text.contains("image/jpeg"), "the image is named");

    // The evidence keeps the original — that is the point of ADR-0004.
    let raw_has_payload: i64 = db
        .conn()
        .query_row(
            "SELECT count(*) FROM raw_event WHERE body LIKE '%QUJDRA0KQUJDRA0K%'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(raw_has_payload, 1, "raw_event is untouched");
}

/// The attribution the corpus investigation corrected.
#[test]
fn subagent_calls_are_attributed_to_their_agent() {
    let db = ingested();

    for id in ["toolu_side_1", "toolu_side_2"] {
        let call = query::tool_call_detail(db.conn(), id)
            .expect("q")
            .expect("row");
        assert_eq!(call.is_sidechain, Some(true));
        assert_eq!(call.agent_id.as_deref(), Some("aTESTAGENT00001"));
        assert_eq!(
            call.agent_name.as_deref(),
            Some("Explore"),
            "type spread across the instance, including to records that never carried it"
        );
    }

    // The spawning call is main-thread work and must not be attributed.
    let spawn = query::tool_call_detail(db.conn(), "toolu_spawn")
        .expect("q")
        .expect("row");
    assert_eq!(spawn.is_sidechain, Some(false));
    assert_eq!(spawn.agent_id, None);
    assert_eq!(spawn.tool_kind.as_deref(), Some("agent"));
    assert_eq!(spawn.target_path.as_deref(), Some("Explore"));
}

/// `agent-name` is a session label, not subagent attribution — the distinction
/// the plan had wrong.
#[test]
fn agent_name_labels_the_session_not_the_subagent() {
    let db = ingested();
    let sessions = query::list_sessions(db.conn(), Page::default()).expect("sessions");
    let s = sessions
        .iter()
        .find(|s| s.session_id == "s-agent")
        .expect("session");

    assert_eq!(s.agent_name.as_deref(), Some("add-healthcheck-endpoint"));
    assert_ne!(s.agent_name.as_deref(), Some("Explore"));
}

#[test]
fn relocated_records_move_the_session_directory() {
    let db = ingested();
    let sessions = query::list_sessions(db.conn(), Page::default()).expect("sessions");
    let s = sessions
        .iter()
        .find(|s| s.session_id == "s-edge")
        .expect("session");

    assert_eq!(
        s.cwd.as_deref(),
        Some("/work/moved-app"),
        "the move wins over the old cwd"
    );
}

/// New tools ship constantly; one this build has never heard of must still
/// produce a usable row.
#[test]
fn an_unknown_tool_still_lands_a_usable_row() {
    let db = ingested();
    let call = query::tool_call_detail(db.conn(), "toolu_future")
        .expect("q")
        .expect("row");

    assert_eq!(call.tool_name.as_deref(), Some("QuantumEntangler"));
    assert_eq!(call.tool_kind.as_deref(), Some("builtin"));

    // Its argument keys are alien too, so there is no path to extract. The row
    // is still usable: the summary falls back to compact JSON and the full
    // input is kept verbatim.
    assert_eq!(call.target_path, None);
    let summary = call.input_summary.as_deref().expect("a summary regardless");
    assert!(summary.contains("entanglement_depth"), "got {summary}");
    assert!(
        call.input_json
            .as_deref()
            .expect("input")
            .contains("/work/app/src/lib.rs")
    );
}

#[test]
fn search_finds_commands_paths_and_result_text() {
    let db = ingested();

    let hits = query::search(db.conn(), "cargo", Page::default()).expect("search");
    assert!(
        hits.iter()
            .any(|h| h.tool_call.tool_use_id == "toolu_bash_ok")
    );

    assert!(
        !query::search(db.conn(), "auth.rs", Page::default())
            .expect("search")
            .is_empty(),
        "file paths are searchable"
    );
    assert!(
        !query::search(db.conn(), "healthcheck", Page::default())
            .expect("search")
            .is_empty(),
        "result text is searchable"
    );
}

#[test]
fn timeline_orders_newest_first_and_filters_by_sidechain() {
    let db = ingested();

    let all = query::timeline_page(db.conn(), &TimelineFilter::default(), Page::default())
        .expect("timeline");
    assert!(all.len() >= 12);
    for pair in all.windows(2) {
        assert!(pair[0].called_at >= pair[1].called_at, "newest first");
    }

    let side = TimelineFilter {
        is_sidechain: Some(true),
        ..TimelineFilter::default()
    };
    assert_eq!(query::timeline_count(db.conn(), &side).expect("count"), 2);
}

/// Re-projection must reproduce the same rows from evidence alone — including
/// the subagent attribution, which depends on the whole stream.
#[test]
fn reprojection_reproduces_identical_rows() {
    let db = ingested();
    let before = query::stats_totals(db.conn()).expect("totals");
    let agent_before = query::tool_call_detail(db.conn(), "toolu_side_1")
        .expect("q")
        .expect("row");

    let mut projector = toolog_ingest::TranscriptProjector::new();
    toolog_core::project::reproject(db.conn(), None, &mut projector).expect("reproject");

    let after = query::stats_totals(db.conn()).expect("totals");
    let agent_after = query::tool_call_detail(db.conn(), "toolu_side_1")
        .expect("q")
        .expect("row");

    assert_eq!(before.tool_calls, after.tool_calls);
    assert_eq!(before.file_changes, after.file_changes);
    assert_eq!(agent_before.agent_name, agent_after.agent_name);
    assert_eq!(format!("{agent_before:?}"), format!("{agent_after:?}"));
}
