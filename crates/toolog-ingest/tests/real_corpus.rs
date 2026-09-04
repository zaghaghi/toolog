//! Regression coverage against the machine's own transcripts.
//!
//! Nothing here is committed. `fixtures/` holds synthetic structures because the
//! real corpus is 37 MB of prompts, source code and client project names, and
//! this repository is public. What real data still buys is breadth — 12 Claude
//! Code versions and 21 record types, including shapes nobody thought to invent.
//!
//! So these tests read `~/.claude/projects` when it exists and assert on
//! *properties* rather than values, and skip cleanly when it does not, so CI
//! stays green on a machine that has never run Claude Code.

use toolog_core::model::TimelineFilter;
use toolog_core::{Db, query};
use toolog_ingest::{Backfill, discover};

/// Ingest the local corpus, or `None` if there isn't one.
fn corpus() -> Option<(Db, toolog_ingest::BackfillReport)> {
    let root = discover::projects_dir()?;
    if !root.is_dir() || discover::transcripts(&root).is_empty() {
        return None;
    }
    let db = Db::open_in_memory().expect("open");
    let report = Backfill::new(db.conn()).run(&root).expect("backfill");
    Some((db, report))
}

macro_rules! corpus_or_skip {
    () => {
        match corpus() {
            Some(v) => v,
            None => {
                eprintln!("skipping: no ~/.claude/projects on this machine");
                return;
            }
        }
    };
}

/// The headline exit criterion: a real backfill parses cleanly.
#[test]
fn the_real_corpus_parses_without_a_single_failure() {
    let (db, report) = corpus_or_skip!();

    assert_eq!(
        report.stats.unparsable, 0,
        "every line in the corpus is valid JSON"
    );
    assert!(
        report.stats.tool_uses > 1000,
        "got {}",
        report.stats.tool_uses
    );

    let totals = query::stats_totals(db.conn()).expect("totals");
    assert!(totals.tool_calls > 1000, "got {}", totals.tool_calls);

    eprintln!(
        "corpus: {} files, {} lines, {} tool calls, {} sessions, {} unknown record types",
        report.files,
        report.lines,
        totals.tool_calls,
        totals.sessions,
        report.stats.unknown_records
    );
}

/// Every tool in the corpus normalizes to something displayable, whether or not
/// this build has heard of it.
#[test]
fn every_tool_gets_a_name_and_a_kind() {
    let (db, _) = corpus_or_skip!();

    let unnamed: i64 = db
        .conn()
        .query_row(
            "SELECT count(*) FROM tool_call WHERE provenance = 1 AND (tool_name IS NULL OR tool_kind IS NULL)",
            [],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(
        unnamed, 0,
        "no transcript-derived call lacks a name or kind"
    );

    let usage = query::stats_tool_usage(db.conn()).expect("usage");
    assert!(
        usage.len() > 10,
        "a real corpus exercises many tools, saw {}",
        usage.len()
    );
    eprintln!("tools seen: {}", usage.len());
}

/// The property the corpus investigation established: `agentId` is present on
/// every sidechain record and no main-thread one.
#[test]
fn sidechain_calls_all_carry_an_agent_id() {
    let (db, _) = corpus_or_skip!();

    let (side, side_with_id, main_with_id): (i64, i64, i64) = db
        .conn()
        .query_row(
            "SELECT
                 sum(CASE WHEN is_sidechain = 1 THEN 1 ELSE 0 END),
                 sum(CASE WHEN is_sidechain = 1 AND agent_id IS NOT NULL THEN 1 ELSE 0 END),
                 sum(CASE WHEN is_sidechain = 0 AND agent_id IS NOT NULL THEN 1 ELSE 0 END)
             FROM tool_call",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("query");

    if side == 0 {
        eprintln!("skipping: this corpus has no subagent activity");
        return;
    }
    assert_eq!(side_with_id, side, "every sidechain call has an agent_id");
    assert_eq!(main_with_id, 0, "no main-thread call has one");
    eprintln!("sidechain calls: {side}, all attributed");
}

/// A second pass must create no duplicates.
///
/// Note it does not assert the second pass stores *nothing*: on a developer's
/// own machine the corpus is live, and a Claude Code session running while this
/// test does will append records between the two passes. The invariant that
/// actually matters is that re-reading what is already held adds nothing.
#[test]
fn re_ingesting_the_corpus_creates_no_duplicates() {
    let Some(root) = discover::projects_dir() else {
        return;
    };
    if !root.is_dir() || discover::transcripts(&root).is_empty() {
        eprintln!("skipping: no ~/.claude/projects on this machine");
        return;
    }

    let db = Db::open_in_memory().expect("open");
    Backfill::new(db.conn()).run(&root).expect("first");
    let before = query::stats_totals(db.conn()).expect("totals");

    let second = Backfill::new(db.conn()).run(&root).expect("second");
    let after = query::stats_totals(db.conn()).expect("totals");

    assert!(second.duplicates > 0, "the corpus was already held");
    assert_eq!(
        after.raw_events,
        before.raw_events + i64::try_from(second.stored).expect("fits"),
        "the database grew by exactly the genuinely new lines, and no more"
    );
    assert!(after.tool_calls >= before.tool_calls, "nothing was lost");
}

/// Re-projection from evidence alone reproduces the same rows, at real scale.
#[test]
fn reprojecting_the_corpus_is_stable() {
    let (db, _) = corpus_or_skip!();
    let before = query::stats_totals(db.conn()).expect("totals");
    let filter = TimelineFilter::default();
    let count_before = query::timeline_count(db.conn(), &filter).expect("count");

    let mut projector = toolog_ingest::TranscriptProjector::new();
    toolog_core::project::reproject(db.conn(), None, &mut projector).expect("reproject");

    let after = query::stats_totals(db.conn()).expect("totals");
    assert_eq!(before.tool_calls, after.tool_calls);
    assert_eq!(before.file_changes, after.file_changes);
    assert_eq!(before.sessions, after.sessions);
    assert_eq!(
        count_before,
        query::timeline_count(db.conn(), &filter).expect("count")
    );
}
