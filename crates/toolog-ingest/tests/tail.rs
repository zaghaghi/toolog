//! Live-tail behaviour: the Phase 2 exit criteria that need a real file being
//! written while it is read.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use toolog_core::model::{Page, TimelineFilter};
use toolog_core::{Db, query};
use toolog_ingest::backfill::ingest_file;
use toolog_ingest::projector::TranscriptProjector;
use toolog_ingest::tail::Tail;

/// One assistant record carrying a single Bash tool call.
fn record(n: usize) -> String {
    format!(
        r#"{{"type":"assistant","uuid":"u-{n}","sessionId":"s-tail","timestamp":"2026-06-01T10:00:0{}.000Z","cwd":"/work/app","version":"2.1.259","isSidechain":false,"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_{n}","name":"Bash","input":{{"command":"echo {n}"}}}}]}}}}"#,
        n % 10
    )
}

fn wait_until(deadline: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    done()
}

/// Exit criterion: appending while the tailer runs produces exactly one new row
/// per tool call.
#[test]
fn appending_while_tailing_yields_one_row_per_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("t.db");
    let watch = dir.path().join("projects");
    std::fs::create_dir_all(&watch).expect("mkdir");
    let transcript = watch.join("session.jsonl");
    std::fs::write(&transcript, format!("{}\n", record(0))).expect("seed");

    let stop = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(AtomicUsize::new(0));

    let worker = {
        let (stop, seen, db_path, watch) =
            (stop.clone(), seen.clone(), db_path.clone(), watch.clone());
        std::thread::spawn(move || {
            let db = Db::open(&db_path).expect("open");
            Tail::new(&watch)
                .with_debounce(Duration::from_millis(80))
                .run(
                    db.conn(),
                    |_| {
                        seen.fetch_add(1, Ordering::SeqCst);
                    },
                    &|| stop.load(Ordering::SeqCst),
                )
                .expect("tail");
        })
    };

    // Append in bursts, the way Claude Code writes.
    for batch in 1..=3 {
        let mut chunk = String::new();
        for n in (batch * 10)..(batch * 10 + 3) {
            chunk.push_str(&record(n));
            chunk.push('\n');
        }
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&transcript)
                .expect("open append");
            f.write_all(chunk.as_bytes()).expect("append");
            f.flush().expect("flush");
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    let reader = Db::open(&db_path).expect("reader");
    let counted = wait_until(Duration::from_secs(10), || {
        query::timeline_count(reader.conn(), &TimelineFilter::default()).unwrap_or(0) >= 10
    });

    stop.store(true, Ordering::SeqCst);
    // Nudge the watcher so its recv_timeout returns promptly.
    std::fs::write(watch.join("nudge.jsonl"), "").ok();
    worker.join().expect("join");

    assert!(
        counted,
        "expected 10 tool calls, saw {:?}",
        query::stats_totals(reader.conn())
    );

    let rows = query::timeline_page(
        reader.conn(),
        &TimelineFilter::default(),
        Page {
            limit: 100,
            offset: 0,
        },
    )
    .expect("timeline");
    assert_eq!(rows.len(), 10, "one row per call, no duplicates");

    let mut ids: Vec<_> = rows.iter().map(|r| r.tool_use_id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 10, "tool_use_ids are unique");
    assert!(seen.load(Ordering::SeqCst) > 0, "progress was reported");
}

/// Exit criterion: killing the tailer mid-file and restarting loses nothing and
/// duplicates nothing.
#[test]
fn interrupting_and_resuming_loses_and_duplicates_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transcript = dir.path().join("session.jsonl");

    let all: String = (0..6).fold(String::new(), |mut acc, n| {
        acc.push_str(&record(n));
        acc.push('\n');
        acc
    });
    let split = all.find('\n').expect("newline") + 1;
    let (head, tail_text) = all.split_at(split * 3);

    std::fs::write(&transcript, head).expect("write head");

    let db = Db::open(dir.path().join("t.db")).expect("open");
    let first = ingest_file(db.conn(), &transcript, None).expect("first pass");
    assert_eq!(first.stored, 3);

    // "Crash" here, then the file grows, then a fresh pass resumes.
    std::fs::write(&transcript, format!("{head}{tail_text}")).expect("append rest");
    let second = ingest_file(db.conn(), &transcript, None).expect("resume");

    let mut projector = TranscriptProjector::new();
    toolog_core::project::reproject(db.conn(), None, &mut projector).expect("project");

    assert_eq!(second.stored, 3, "only the new lines were stored");
    let totals = query::stats_totals(db.conn()).expect("totals");
    assert_eq!(totals.raw_events, 6, "nothing lost, nothing duplicated");
    assert_eq!(totals.tool_calls, 6);
}

/// A file being rewritten shorter invalidates the stored offset; recovery is to
/// rescan from zero and let deduplication absorb the overlap.
#[test]
fn a_truncated_file_is_rescanned_from_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transcript = dir.path().join("session.jsonl");
    let long: String = (0..6).fold(String::new(), |mut acc, n| {
        acc.push_str(&record(n));
        acc.push('\n');
        acc
    });
    std::fs::write(&transcript, &long).expect("write");

    let db = Db::open(dir.path().join("t.db")).expect("open");
    assert_eq!(
        ingest_file(db.conn(), &transcript, None)
            .expect("first")
            .stored,
        6
    );

    // Rewritten shorter, keeping one original line and adding a new one.
    let rewritten = format!("{}\n{}\n", record(0), record(99));
    std::fs::write(&transcript, rewritten).expect("truncate");

    let after = ingest_file(db.conn(), &transcript, None).expect("rescan");
    assert_eq!(
        after.lines, 2,
        "rescanned from zero rather than trusting the offset"
    );
    assert_eq!(after.stored, 1, "only the genuinely new line was stored");

    let totals = query::stats_totals(db.conn()).expect("totals");
    assert_eq!(totals.raw_events, 7, "the six originals plus one new");
}

/// A record still being written must never be stored — evidence is the one
/// thing that cannot be half-right.
#[test]
fn a_partially_written_record_is_not_stored_until_complete() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transcript = dir.path().join("session.jsonl");
    let complete = record(1);

    std::fs::write(&transcript, format!("{complete}\n{}", &record(2)[..40])).expect("partial");

    let db = Db::open(dir.path().join("t.db")).expect("open");
    let first = ingest_file(db.conn(), &transcript, None).expect("first");
    assert_eq!(first.stored, 1, "only the terminated line");
    assert!(first.trailing_partial, "the fragment was noticed");

    // Once the writer finishes the line, it lands — exactly once.
    std::fs::write(&transcript, format!("{complete}\n{}\n", record(2))).expect("complete");
    let second = ingest_file(db.conn(), &transcript, None).expect("second");
    assert_eq!(second.stored, 1);

    let mut projector = TranscriptProjector::new();
    toolog_core::project::reproject(db.conn(), None, &mut projector).expect("project");
    assert_eq!(
        projector.stats().unparsable,
        0,
        "no truncated JSON was ever stored"
    );
    assert_eq!(
        query::stats_totals(db.conn()).expect("totals").tool_calls,
        2
    );
}

/// Bursts are coalesced rather than acted on per event.
#[test]
fn a_write_burst_is_debounced_into_one_ingest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let watch = dir.path().join("projects");
    std::fs::create_dir_all(&watch).expect("mkdir");
    let transcript = watch.join("session.jsonl");
    std::fs::write(&transcript, "").expect("seed");

    let stop = Arc::new(AtomicBool::new(false));
    let reports = Arc::new(Mutex::new(Vec::new()));

    let worker = {
        let (stop, reports, watch, db_path) = (
            stop.clone(),
            reports.clone(),
            watch.clone(),
            dir.path().join("t.db"),
        );
        std::thread::spawn(move || {
            let db = Db::open(&db_path).expect("open");
            Tail::new(&watch)
                .with_debounce(Duration::from_millis(250))
                .run(
                    db.conn(),
                    |r| reports.lock().expect("lock").push(r.stored),
                    &|| stop.load(Ordering::SeqCst),
                )
                .expect("tail");
        })
    };

    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .expect("open");
        for n in 0..8 {
            writeln!(f, "{}", record(n)).expect("write");
            f.flush().expect("flush");
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    wait_until(Duration::from_secs(6), || {
        reports.lock().expect("lock").iter().sum::<usize>() >= 8
    });
    stop.store(true, Ordering::SeqCst);
    std::fs::write(watch.join("nudge.jsonl"), "").ok();
    worker.join().expect("join");

    let calls = reports.lock().expect("lock").clone();
    assert!(
        calls.iter().sum::<usize>() >= 8,
        "all records ingested: {calls:?}"
    );
    assert!(
        calls.len() < 8,
        "eight writes coalesced into fewer ingests, got {} for {calls:?}",
        calls.len()
    );
}
