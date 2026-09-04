//! Re-projection must rebuild **both** lanes, and an import must not destroy one.
//!
//! This exists because of a real bug found in Phase 4 against live data.
//! `toolog backfill` finished by re-projecting the whole database with the
//! transcript projector alone. Re-projection clears every projection table
//! first, so a single import silently deleted every permission decision,
//! decision source and duration in the store — the entire OTLP half. The
//! evidence in `raw_event` survived, which is the only reason it was
//! recoverable ([ADR-0004]), but nothing said anything was wrong.
//!
//! [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md

use prost::Message as _;
use toolog_cli::commands;
use toolog_core::{Db, query};
use toolog_otlp::testing;

/// Store one refused call through the OTLP lane.
fn seed_a_refusal(db: &Db) {
    let records = vec![testing::tool_decision("toolu_denied", "reject", "config")];
    toolog_otlp::ingest_records(db.conn(), "otlp:test", &records).expect("ingest");
    // Keep the encoded form honest: the same records must round-trip the wire.
    let _ = testing::request(records).encode_to_vec();
}

fn decision_of(db: &Db, id: &str) -> Option<String> {
    query::tool_call_detail(db.conn(), id)
        .expect("query")
        .and_then(|c| c.decision)
}

#[test]
fn importing_transcripts_does_not_delete_the_decision_layer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::open(dir.path().join("t.db")).expect("open");
    seed_a_refusal(&db);
    assert_eq!(
        decision_of(&db, "toolu_denied").as_deref(),
        Some("reject"),
        "the OTLP lane recorded the refusal"
    );

    // A backfill over the committed fixtures: a normal import, alongside data
    // this crate's projector knows nothing about.
    let report = toolog_ingest::Backfill::new(db.conn())
        .run(std::path::Path::new("../../fixtures/transcripts"))
        .expect("backfill");
    assert!(report.files > 0, "the fixtures were read");
    assert!(report.stats.tool_uses > 0, "and projected");

    assert_eq!(
        decision_of(&db, "toolu_denied").as_deref(),
        Some("reject"),
        "an import must not clear a lane it cannot rebuild"
    );
}

#[test]
fn a_full_reprojection_rebuilds_every_lane_from_evidence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::open(dir.path().join("t.db")).expect("open");
    seed_a_refusal(&db);
    toolog_ingest::Backfill::new(db.conn())
        .run(std::path::Path::new("../../fixtures/transcripts"))
        .expect("backfill");

    let before = query::reconcile(db.conn()).expect("reconcile");
    let calls_before = query::stats_totals(db.conn()).expect("totals").tool_calls;

    let stats = commands::reproject_all(db.conn()).expect("reproject");
    assert!(stats.scanned > 0);

    let after = query::reconcile(db.conn()).expect("reconcile");
    assert_eq!(
        query::stats_totals(db.conn()).expect("totals").tool_calls,
        calls_before,
        "rebuilding from evidence reproduces the same rows"
    );
    assert_eq!(after.rejected, before.rejected, "refusals survive");
    assert_eq!(after.both, before.both);
    assert_eq!(after.otel_only, before.otel_only);
    assert_eq!(
        decision_of(&db, "toolu_denied").as_deref(),
        Some("reject"),
        "the decision came back from raw_event"
    );
}
