//! A secret put through the real pipeline (tasks 7.2 and 7.3).
//!
//! The phase's exit criterion is one sentence: *a deliberately introduced secret
//! in a test transcript is redacted in the projection.* The first test here is
//! that sentence, and the second is its other half — that the **evidence** still
//! has it, which is the trade-off task 7.3 exists to make explicit rather than
//! silent.
//!
//! Every "secret" below is a syntactically valid but invented value. They are
//! shaped to match the patterns and belong to nothing.

use std::io::Write as _;

use toolog_core::model::{Page, TimelineFilter};
use toolog_core::{Connection, Db, query};
use toolog_ingest::Backfill;

/// A transcript whose one command carries a secret, ingested.
fn ingest(command: &str) -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut file = std::fs::File::create(&path).expect("create");

    let escaped = serde_json::to_string(command).expect("json string");
    let record = format!(
        r#"{{"type":"assistant","uuid":"u1","sessionId":"s1","timestamp":"2026-09-05T09:00:00.000Z","cwd":"/work","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_1","name":"Bash","input":{{"command":{escaped}}}}}]}}}}"#
    );
    writeln!(file, "{record}").expect("write");
    drop(file);

    let db = Db::open_in_memory().expect("open");
    Backfill::new(db.conn()).run(dir.path()).expect("backfill");
    (dir, db)
}

fn summary_of(conn: &Connection, tool_use_id: &str) -> String {
    query::tool_call_detail(conn, tool_use_id)
        .expect("detail")
        .expect("the call")
        .input_summary
        .unwrap_or_default()
}

fn evidence(conn: &Connection) -> String {
    conn.query_row("SELECT group_concat(body, '\n') FROM raw_event", [], |r| {
        r.get::<_, Option<String>>(0)
    })
    .expect("bodies")
    .unwrap_or_default()
}

/// The exit criterion.
#[test]
fn a_secret_in_a_transcript_is_redacted_in_the_projection() {
    let secret = "AKIAIOSFODNN7EXAMPLE";
    let (_dir, db) = ingest(&format!("aws s3 ls --profile {secret}"));

    let summary = summary_of(db.conn(), "toolu_1");
    assert!(
        !summary.contains(secret),
        "the key survived into the projection: {summary}"
    );
    assert_eq!(summary, "aws s3 ls --profile [redacted: aws-access-key-id]");
}

/// Task 7.3's decision, as an assertion rather than a paragraph: by default the
/// evidence store keeps what the projection hides.
#[test]
fn by_default_the_evidence_still_holds_the_secret_and_that_is_deliberate() {
    let secret = "AKIAIOSFODNN7EXAMPLE";
    let (_dir, db) = ingest(&format!("aws s3 ls --profile {secret}"));

    assert!(
        evidence(db.conn()).contains(secret),
        "ADR-0004 makes raw_event the thing every projection is rebuilt from, so \
         it is not redacted unless the user asks — and PRIVACY.md says so"
    );
}

/// A re-projection is where a wrong pattern gets fixed, so it has to redact too.
#[test]
fn a_re_projection_redacts_again_rather_than_restoring_the_secret() {
    let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let (_dir, db) = ingest(&format!(
        "git remote set-url origin https://{secret}@github.com/x/y"
    ));
    assert!(!summary_of(db.conn(), "toolu_1").contains(secret));

    let mut projector = toolog_ingest::TranscriptProjector::new();
    toolog_core::project::reproject(db.conn(), None, &mut projector).expect("reproject");

    let summary = summary_of(db.conn(), "toolu_1");
    assert!(
        !summary.contains(secret),
        "rebuilding from evidence must redact on the way out too: {summary}"
    );
    assert!(summary.contains("[redacted:"));
}

#[test]
fn the_stored_arguments_are_redacted_not_only_the_summary() {
    let secret = "hunter2xyzsecret";
    let (_dir, db) = ingest(&format!("psql 'host=db password={secret}'"));

    let call = query::tool_call_detail(db.conn(), "toolu_1")
        .expect("detail")
        .expect("the call");
    let input_json = call.input_json.unwrap_or_default();
    assert!(
        !input_json.contains(secret),
        "input_json is stored verbatim in the projection too: {input_json}"
    );
    assert!(input_json.contains("[redacted: password-assignment]"));
}

/// Redaction must not make a row unsearchable by everything *except* the
/// secret — the command is still the command.
#[test]
fn a_redacted_row_is_still_findable_by_what_it_did() {
    let (_dir, db) = ingest("aws s3 ls --profile AKIAIOSFODNN7EXAMPLE");

    let hits = query::search(db.conn(), "s3", Page::default()).expect("search");
    assert_eq!(hits.len(), 1, "the command is still indexed");

    let filtered = query::timeline_rows(
        db.conn(),
        &TimelineFilter {
            query: Some("aws".to_string()),
            ..TimelineFilter::default()
        },
        Page::default(),
    )
    .expect("rows");
    assert_eq!(filtered.len(), 1);
}

/// A command with nothing secret in it must come through untouched — an
/// over-eager redactor that mangles ordinary commands is unusable.
#[test]
fn an_ordinary_command_is_stored_exactly_as_it_ran() {
    let command = "cargo test --workspace && git status --short";
    let (_dir, db) = ingest(command);
    assert_eq!(summary_of(db.conn(), "toolu_1"), command);
}
