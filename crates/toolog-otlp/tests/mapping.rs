//! Golden payloads to rows, and the ordering property both lanes depend on.

use opentelemetry_proto::tonic::logs::v1::LogRecord;
use prost::Message;
use toolog_core::model::{Page, TimelineFilter, TranscriptFacts, provenance};
use toolog_core::{Db, project, query};
use toolog_otlp::decode::{self, Encoding};
use toolog_otlp::{ingest_records, testing};

fn ingest(db: &Db, records: &[LogRecord]) -> toolog_otlp::IngestStats {
    ingest_records(db.conn(), "otlp:test", records).expect("ingest")
}

/// The same batch over either wire format must produce the same rows.
#[test]
fn both_encodings_produce_identical_rows() {
    let request = testing::request(vec![
        testing::tool_result("toolu_1", true, 42),
        testing::api_request("req_1", 12_345),
    ]);

    let dump = |bytes: &[u8], encoding| {
        let db = Db::open_in_memory().expect("open");
        let records = decode::records(decode::logs(encoding, bytes).expect("decode"));
        ingest(&db, &records);
        let call = query::tool_call_detail(db.conn(), "toolu_1")
            .expect("q")
            .expect("row");
        let totals = query::stats_totals(db.conn()).expect("totals");
        format!("{call:?}|{totals:?}")
    };

    let json = dump(&serde_json::to_vec(&request).expect("json"), Encoding::Json);
    let proto = dump(&request.encode_to_vec(), Encoding::Protobuf);
    assert_eq!(json, proto);
}

#[test]
fn a_tool_result_lands_duration_and_decision_source() {
    let db = Db::open_in_memory().expect("open");
    let stats = ingest(&db, &[testing::tool_result("toolu_1", true, 42)]);

    assert_eq!(stats.stored, 1);
    assert_eq!(stats.projected.tool_results, 1);

    let call = query::tool_call_detail(db.conn(), "toolu_1")
        .expect("q")
        .expect("row");
    assert_eq!(call.tool_name.as_deref(), Some("Bash"));
    assert_eq!(call.duration_ms, Some(42));
    assert_eq!(call.decision_source.as_deref(), Some("config"));
    assert_eq!(call.success, Some(true));
    assert_eq!(call.provenance, provenance::OTLP);
    assert_eq!(call.session_id.as_deref(), Some("s-otel"));
    assert_eq!(call.prompt_id.as_deref(), Some("p-1"));

    // The session was established from the event's own attributes.
    let sessions = query::list_sessions(db.conn(), Page::default()).expect("sessions");
    let s = sessions
        .iter()
        .find(|s| s.session_id == "s-otel")
        .expect("session");
    assert_eq!(s.cc_version.as_deref(), Some("2.1.260"));
    assert_eq!(
        s.project_path.as_deref(),
        Some("/work/app"),
        "from workspace.host_paths"
    );
}

/// The reason this lane exists.
#[test]
fn a_rejected_call_is_captured_with_no_transcript_body() {
    let db = Db::open_in_memory().expect("open");
    let stats = ingest(
        &db,
        &[testing::tool_decision(
            "toolu_denied",
            "reject",
            "user_reject",
        )],
    );

    assert_eq!(stats.projected.rejections, 1);

    let call = query::tool_call_detail(db.conn(), "toolu_denied")
        .expect("q")
        .expect("row");
    assert!(call.is_rejected());
    assert_eq!(call.decision.as_deref(), Some("reject"));
    assert_eq!(call.decision_source.as_deref(), Some("user_reject"));
    assert_eq!(call.provenance, provenance::OTLP, "OTLP alone witnessed it");
    assert_eq!(call.success, Some(false));
    assert!(
        call.input_json.is_none() && call.result_json.is_none(),
        "a denied call never ran, so there is no transcript content to have"
    );

    // Reconciliation must read it as a rejection, not a gap.
    let r = query::reconcile(db.conn()).expect("reconcile");
    assert_eq!(r.otel_only, 1);
    assert_eq!(r.transcript_only, 0);
}

#[test]
fn decision_sources_are_preserved_verbatim() {
    let db = Db::open_in_memory().expect("open");
    // Every source Claude Code can report; the risk view distinguishes them.
    for (id, decision, source) in [
        ("t_cfg", "accept", "config"),
        ("t_hook", "accept", "hook"),
        ("t_perm", "accept", "user_permanent"),
        ("t_temp", "accept", "user_temporary"),
        ("t_abort", "reject", "user_abort"),
        ("t_rej", "reject", "user_reject"),
    ] {
        ingest(&db, &[testing::tool_decision(id, decision, source)]);
        let call = query::tool_call_detail(db.conn(), id)
            .expect("q")
            .expect("row");
        assert_eq!(call.decision_source.as_deref(), Some(source));
        assert_eq!(call.decision.as_deref(), Some(decision));
    }

    let auto = TimelineFilter {
        decision_source: Some("config".into()),
        ..TimelineFilter::default()
    };
    assert_eq!(query::timeline_count(db.conn(), &auto).expect("count"), 1);
}

/// The core correctness property of ADR-0009, across real lanes this time.
#[test]
fn lane_arrival_order_does_not_change_the_row() {
    let transcript = TranscriptFacts {
        session_id: Some("s-otel".into()),
        tool_name: Some("Bash".into()),
        input_summary: Some("a command far longer than OTEL would ever carry".into()),
        input_json: Some(r#"{"command":"..."}"#.into()),
        called_at: Some(1_700_000_000_000),
        success: Some(true),
        ..TranscriptFacts::default()
    };

    let build = |otel_first: bool| {
        let db = Db::open_in_memory().expect("open");
        if otel_first {
            ingest(&db, &[testing::tool_result("toolu_1", true, 42)]);
            project::upsert_transcript(db.conn(), "toolu_1", &transcript).expect("transcript");
        } else {
            project::upsert_transcript(db.conn(), "toolu_1", &transcript).expect("transcript");
            ingest(&db, &[testing::tool_result("toolu_1", true, 42)]);
        }
        query::tool_call_detail(db.conn(), "toolu_1")
            .expect("q")
            .expect("row")
    };

    let a = build(true);
    let b = build(false);

    assert_eq!(
        format!("{a:?}"),
        format!("{b:?}"),
        "arrival order must not matter"
    );
    assert_eq!(a.provenance, provenance::BOTH);
    assert_eq!(a.duration_ms, Some(42), "from OTLP");
    assert_eq!(
        a.input_summary.as_deref(),
        Some("a command far longer than OTEL would ever carry"),
        "OTLP must never overwrite transcript content"
    );
}

#[test]
fn api_requests_carry_cost_and_tokens() {
    let db = Db::open_in_memory().expect("open");
    ingest(&db, &[testing::api_request("req_1", 12_345)]);

    let totals = query::stats_totals(db.conn()).expect("totals");
    assert_eq!(totals.api_requests, 1);
    assert_eq!(totals.cost_usd_micros, 12_345);
    assert_eq!(totals.input_tokens, 100);
    assert_eq!(totals.output_tokens, 200);
}

/// A retried export must not double-count.
#[test]
fn a_replayed_batch_is_stored_once() {
    let db = Db::open_in_memory().expect("open");
    let records = [testing::tool_result("toolu_1", true, 42)];

    let first = ingest(&db, &records);
    let second = ingest(&db, &records);

    assert_eq!(first.stored, 1);
    assert_eq!(second.stored, 0, "the content hash matched");
    assert_eq!(
        second.projected.tool_results, 0,
        "and it was not projected again"
    );
    assert_eq!(
        query::stats_totals(db.conn()).expect("totals").tool_calls,
        1
    );
}

/// An event type from a future Claude Code release is kept, not dropped.
#[test]
fn unknown_events_are_stored_for_later_projection() {
    let db = Db::open_in_memory().expect("open");
    let stats = ingest(
        &db,
        &[testing::record(
            "quantum_entanglement",
            1_700_000_000_000,
            vec![
                testing::s("session.id", "s-otel"),
                testing::s("mystery", "yes"),
            ],
        )],
    );

    assert_eq!(stats.stored, 1, "kept as evidence");
    assert_eq!(stats.projected.other, 1, "counted as unprojected");
    assert_eq!(
        query::stats_totals(db.conn()).expect("totals").raw_events,
        1
    );

    // The attribute survives in evidence, so a later build can project it.
    let body: String = db
        .conn()
        .query_row("SELECT body FROM raw_event", [], |r| r.get(0))
        .expect("body");
    assert!(body.contains("mystery"));
}

/// Re-projection from evidence alone must reproduce the same rows.
#[test]
fn reprojection_from_evidence_is_stable() {
    let db = Db::open_in_memory().expect("open");
    ingest(
        &db,
        &[
            testing::tool_result("toolu_1", true, 42),
            testing::tool_decision("toolu_denied", "reject", "user_reject"),
            testing::api_request("req_1", 999),
        ],
    );
    let before = query::stats_totals(db.conn()).expect("totals");
    let call_before = query::tool_call_detail(db.conn(), "toolu_1")
        .expect("q")
        .expect("row");

    let mut projector = toolog_otlp::OtlpProjector::new();
    project::reproject(db.conn(), None, &mut projector).expect("reproject");

    let after = query::stats_totals(db.conn()).expect("totals");
    assert_eq!(before.tool_calls, after.tool_calls);
    assert_eq!(before.api_requests, after.api_requests);
    assert_eq!(before.cost_usd_micros, after.cost_usd_micros);
    assert_eq!(
        format!("{call_before:?}"),
        format!(
            "{:?}",
            query::tool_call_detail(db.conn(), "toolu_1")
                .expect("q")
                .expect("row")
        )
    );
}

/// Prompt metadata is stored; prompt text has nowhere to go, by construction.
#[test]
fn prompt_events_store_length_but_never_text() {
    let db = Db::open_in_memory().expect("open");
    ingest(
        &db,
        &[testing::record(
            "user_prompt",
            1_700_000_000_000,
            vec![
                testing::s("session.id", "s-otel"),
                testing::s("prompt.id", "p-1"),
                testing::i("prompt_length", 128),
                testing::s("command_name", "review"),
                testing::s("command_source", "builtin"),
                // Present only if a user enabled OTEL_LOG_USER_PROMPTS by hand. There is
                // no column for it, so it cannot be stored even then.
                testing::s("prompt", "my secret prompt text"),
            ],
        )],
    );

    let (length, name): (Option<i64>, Option<String>) = db
        .conn()
        .query_row(
            "SELECT prompt_length, command_name FROM prompt WHERE prompt_id = 'p-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("prompt row");
    assert_eq!(length, Some(128));
    assert_eq!(name.as_deref(), Some("review"));

    let leaked: i64 = db
        .conn()
        .query_row(
            "SELECT count(*) FROM tool_call WHERE COALESCE(input_json, '') LIKE '%secret prompt%'",
            [],
            |r| r.get(0),
        )
        .expect("scan");
    assert_eq!(leaked, 0, "prompt text reaches no projected column");
}

/// Without this, a denied `rm -rf` records only that *a Bash call* was refused —
/// with no indication of what it would have done. There is no transcript for a
/// rejected call, so OTEL's `tool_input` is the sole surviving record.
#[test]
fn a_rejected_call_records_what_was_attempted() {
    let db = Db::open_in_memory().expect("open");
    ingest(
        &db,
        &[testing::tool_decision_with_input(
            "toolu_rm",
            "reject",
            "user_reject",
            "Bash",
            r#"{"command":"rm -rf /important/data","description":"clean up"}"#,
        )],
    );

    let call = query::tool_call_detail(db.conn(), "toolu_rm")
        .expect("q")
        .expect("row");
    assert!(call.is_rejected());
    assert_eq!(call.provenance, provenance::OTLP);
    assert_eq!(
        call.input_summary.as_deref(),
        Some("rm -rf /important/data"),
        "the audit trail must say what was refused, not merely that something was"
    );
    assert!(
        call.input_json
            .as_deref()
            .expect("input")
            .contains("rm -rf")
    );

    // And it is searchable, so the risk view can find it.
    let hits = query::search(db.conn(), "rm -rf", Page::default()).expect("search");
    assert_eq!(hits.len(), 1);
}

/// The mirror of the above: where a transcript exists, its untruncated copy wins.
#[test]
fn otel_input_never_displaces_a_transcript() {
    let full = "echo ".to_string() + &"x".repeat(2000);
    let db = Db::open_in_memory().expect("open");

    project::upsert_transcript(
        db.conn(),
        "toolu_1",
        &TranscriptFacts {
            tool_name: Some("Bash".into()),
            input_summary: Some(full.clone()),
            input_json: Some(format!(r#"{{"command":"{full}"}}"#)),
            ..TranscriptFacts::default()
        },
    )
    .expect("transcript");

    // OTEL arrives later with a truncated copy of the same call.
    ingest(
        &db,
        &[testing::tool_decision_with_input(
            "toolu_1",
            "accept",
            "config",
            "Bash",
            r#"{"command":"echo xxxxx… [truncated]"}"#,
        )],
    );

    let call = query::tool_call_detail(db.conn(), "toolu_1")
        .expect("q")
        .expect("row");
    assert_eq!(
        call.input_summary.as_deref(),
        Some(full.as_str()),
        "the full copy survived"
    );
    assert!(
        !call
            .input_json
            .as_deref()
            .expect("input")
            .contains("truncated")
    );
    assert_eq!(
        call.decision_source.as_deref(),
        Some("config"),
        "OTEL still added its own"
    );
    assert_eq!(call.provenance, provenance::BOTH);
}

/// A rejected call emits a decision and no result, so `tool_parameters` is the
/// only place its target can come from.
///
/// **Not verified against a live refusal** — the account hit its session limit
/// before one could be provoked. The attribute schema is the documented one; a
/// live rejection should be captured when Phase 4 makes `doctor` available.
#[test]
fn a_rejection_recovers_its_target_from_tool_parameters() {
    let db = Db::open_in_memory().expect("open");
    ingest(
        &db,
        &[testing::record(
            "tool_decision",
            1_700_000_000_000,
            vec![
                testing::s("tool_use_id", "toolu_rm"),
                testing::s("tool_name", "Bash"),
                testing::s("decision", "reject"),
                testing::s("source", "user_reject"),
                testing::s("tool_source", "builtin"),
                testing::s(
                    "tool_parameters",
                    r#"{"bash_command":"rm","full_command":"rm -rf /important/data","description":"clean"}"#,
                ),
                testing::s("session.id", "s-otel"),
            ],
        )],
    );

    let call = query::tool_call_detail(db.conn(), "toolu_rm")
        .expect("q")
        .expect("row");
    assert!(call.is_rejected());
    assert_eq!(
        call.input_summary.as_deref(),
        Some("rm -rf /important/data"),
        "the full command, not merely that a Bash call was refused"
    );
    assert_eq!(
        query::search(db.conn(), "rm -rf", Page::default())
            .expect("search")
            .len(),
        1
    );
}
