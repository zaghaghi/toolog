//! The mapper against payloads a real Claude Code session actually sent.
//!
//! `fixtures/otlp/live-session-events.jsonl` holds one exemplar of each event
//! type captured from a live session, with identity and paths scrubbed and the
//! structure left exact. Unlike transcripts — 37 MB of free text that cannot be
//! reliably redacted — an OTLP event's attributes are few and fully enumerable,
//! so these can be committed safely.
//!
//! They exist because assumptions taken from documentation were wrong in ways
//! only live traffic revealed:
//!
//! - the `event.name` attribute carries the **bare** name (`tool_result`) while
//!   the record body carries it qualified (`claude_code.tool_result`)
//! - `plugin_loaded`, `hook_registered` and `assistant_response` arrive without
//!   appearing in the reference this was built from

use toolog_core::model::provenance;
use toolog_core::{Db, query};
use toolog_otlp::OtlpProjector;

fn fixture_lines() -> Vec<String> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/otlp/live-session-events.jsonl");
    std::fs::read_to_string(path)
        .expect("fixture present")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn projected() -> (Db, OtlpProjector) {
    let db = Db::open_in_memory().expect("open");
    let mut projector = OtlpProjector::new();
    for line in fixture_lines() {
        projector.project_body(db.conn(), &line).expect("project");
    }
    (db, projector)
}

#[test]
fn every_live_event_shape_projects_without_failing() {
    let (_db, projector) = projected();
    let stats = projector.stats();

    assert_eq!(stats.unparsable, 0, "every captured record parses");
    assert_eq!(stats.records, fixture_lines().len());
    assert_eq!(stats.tool_results, 1);
    assert_eq!(stats.tool_decisions, 1);
    assert_eq!(
        stats.api_requests, 2,
        "api_request and api_error both land here"
    );
    assert_eq!(stats.prompts, 1);
    assert!(
        stats.other >= 3,
        "assistant_response, plugin_loaded and hook_registered are kept but unprojected, got {}",
        stats.other
    );
}

/// The bug live traffic caught: matching on the documented qualified name
/// silently projected nothing.
#[test]
fn the_real_tool_result_lands_a_complete_row() {
    let (db, _) = projected();
    let rows = query::timeline_page(
        db.conn(),
        &toolog_core::model::TimelineFilter::default(),
        toolog_core::model::Page::default(),
    )
    .expect("timeline");

    assert!(
        !rows.is_empty(),
        "the documented event name would have produced zero rows here"
    );

    // The captured tool_result was for a ToolSearch call; it carries the timing
    // and outcome that exist nowhere else.
    let result = rows
        .iter()
        .find(|c| c.tool_name.as_deref() == Some("ToolSearch"))
        .expect("the tool_result row");
    assert!(
        result.duration_ms.is_some(),
        "duration comes only from this lane"
    );
    assert_eq!(result.success, Some(true));
    assert_eq!(result.provenance, provenance::OTLP);

    // The captured tool_decision was for a Read, and carries who approved it.
    let decision = rows
        .iter()
        .find(|c| c.tool_name.as_deref() == Some("Read"))
        .expect("the decision row");
    assert_eq!(decision.decision.as_deref(), Some("accept"));
    assert_eq!(decision.decision_source.as_deref(), Some("config"));

    // The live tool_decision carried neither tool_input nor tool_parameters, so
    // there is nothing to recover here. The captured tool_result did carry
    // tool_input, which is what fills the gap for an accepted call.
    assert_eq!(decision.input_summary, None);
    assert!(
        result
            .input_summary
            .as_deref()
            .is_some_and(|s| s.contains("select:")),
        "the tool_result's tool_input was captured, got {:?}",
        result.input_summary
    );
}

#[test]
fn real_api_events_carry_cost_and_model() {
    let (db, _) = projected();
    let totals = query::stats_totals(db.conn()).expect("totals");

    assert_eq!(totals.api_requests, 2);
    assert!(totals.cost_usd_micros > 0, "cost exists only in this lane");

    let model: String = db
        .conn()
        .query_row(
            "SELECT model FROM api_request WHERE model IS NOT NULL LIMIT 1",
            [],
            |r| r.get(0),
        )
        .expect("a model");
    assert!(model.starts_with("claude-"), "got {model}");
}

/// ADR-0008: prompt text is redacted by Claude Code because the flag enabling it
/// is deliberately never set. Confirmed against a live payload, not assumed.
#[test]
fn the_live_prompt_event_carries_no_prompt_text() {
    let (db, _) = projected();

    let length: Option<i64> = db
        .conn()
        .query_row("SELECT prompt_length FROM prompt LIMIT 1", [], |r| r.get(0))
        .expect("prompt row");
    assert!(length.is_some_and(|l| l > 0), "length is recorded");

    // Claude Code sends the attribute as "<REDACTED>", and there is no column
    // for it regardless.
    let raw = fixture_lines().join("\n");
    assert!(
        raw.contains("<REDACTED>"),
        "the live payload redacted it at source"
    );
}
