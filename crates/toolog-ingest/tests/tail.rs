//! Live-tail behaviour: the Phase 2 exit criteria that need a real file being
//! written while it is read.
//!
//! **These run one at a time.** Each spins a real filesystem watcher, and six
//! of them in one process contend for FSEvents badly enough that one can see
//! no events at all inside a ten-second budget — which showed up as an
//! intermittent failure of the first test with an empty store, on a machine
//! doing other work. Serialised they finish in about two seconds together.

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

/// Held for the duration of each test, so only one watcher runs at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Take the lock, ignoring poisoning: a panic in one test must fail that test,
/// not cascade into every test after it.
fn exclusive() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
    let _serial = exclusive();
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
    let _serial = exclusive();
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
    let _serial = exclusive();
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
    let _serial = exclusive();
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
    let _serial = exclusive();
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
                // Generous relative to the ~120 ms the writer below spends:
                // the property under test is coalescing, and a debounce that a
                // loaded machine can outrun turns this into a flaky assertion
                // about scheduler timing rather than about the tailer.
                .with_debounce(Duration::from_secs(1))
                // Short enough to rescue an event the filesystem never
                // delivered, which would otherwise hang this test rather than
                // reveal anything about coalescing.
                .with_sweep(Duration::from_millis(500))
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

    wait_until(Duration::from_secs(15), || {
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
    // Count only ingests that stored something: a sweep over an up-to-date file
    // reports zero, and those say nothing about whether the burst coalesced.
    let effective = calls.iter().filter(|stored| **stored > 0).count();
    assert!(
        effective < 8,
        "eight writes coalesced into fewer ingests, got {effective} for {calls:?}"
    );
}

/// The last records of a session must not wait for an event that never comes.
///
/// A burst can be ingested while its final line is still being flushed. The
/// fragment is correctly left unstored — and if the session then ends, no
/// further filesystem event will arrive to collect it. The sweep is what closes
/// that hole, so this writes a record and then deliberately produces no event
/// the watcher could act on.
#[test]
fn a_file_finished_after_the_last_event_is_still_collected() {
    let _serial = exclusive();
    let dir = tempfile::tempdir().expect("tempdir");
    let watch = dir.path().join("projects");
    std::fs::create_dir_all(&watch).expect("mkdir");
    let transcript = watch.join("late.jsonl");
    std::fs::write(&transcript, "").expect("create");

    let db_path = dir.path().join("t.db");
    let stop = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(Mutex::new(0usize));

    let worker = {
        let (stop, seen, db_path, watch) =
            (stop.clone(), seen.clone(), db_path.clone(), watch.clone());
        std::thread::spawn(move || {
            let db = Db::open(&db_path).expect("open");
            Tail::new(&watch)
                .with_debounce(Duration::from_millis(50))
                .with_sweep(Duration::from_millis(150))
                .run(
                    db.conn(),
                    |r| *seen.lock().expect("lock") += r.stored,
                    &|| stop.load(Ordering::SeqCst),
                )
                .expect("tail");
        })
    };

    // Let the watch settle, then append once. Even if the event for this write
    // is lost or arrives at an unhelpful moment, the sweep must find it.
    std::thread::sleep(Duration::from_millis(200));
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .expect("open");
        writeln!(f, "{}", record(0)).expect("write");
        f.flush().expect("flush");
    }

    let collected = wait_until(Duration::from_secs(10), || *seen.lock().expect("lock") >= 1);
    stop.store(true, Ordering::SeqCst);
    std::fs::write(watch.join("nudge.jsonl"), "").ok();
    worker.join().expect("join");

    assert!(collected, "the sweep must collect what no event announced");

    let db = Db::open(&db_path).expect("reopen");
    assert_eq!(
        toolog_core::raw::count(db.conn(), None).expect("count"),
        1,
        "collected exactly once — dedup keeps the sweep from duplicating"
    );
}

/// A file written *before* the watch starts is picked up straight away.
///
/// The hole this closes: a filesystem watcher takes a moment to arm, and
/// anything written in that window produces no event, ever. The safety-net
/// sweep would eventually find it — thirty seconds later, by default, which is
/// thirty seconds of an apparently dead application every time it starts. It
/// showed up as this file's first test failing about one run in four with an
/// entirely empty store.
#[test]
fn what_was_written_before_the_watch_started_is_not_waited_for() {
    let _serial = exclusive();
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("t.db");
    let watch = dir.path().join("projects");
    std::fs::create_dir_all(&watch).expect("mkdir");

    // On disk before anything is watching it.
    let transcript = watch.join("before.jsonl");
    std::fs::write(&transcript, format!("{}\n", record(1))).expect("seed");

    // Opened here first, so the schema is created once: two connections
    // migrating the same new file at the same moment is a race about this
    // test's setup rather than about the tailer.
    let reader = Db::open(&db_path).expect("reader");

    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let (stop, db_path, watch) = (stop.clone(), db_path.clone(), watch.clone());
        std::thread::spawn(move || {
            let db = Db::open(&db_path).expect("open");
            // A sweep interval far longer than the assertion below, so passing
            // cannot be the safety net coming round on its own schedule.
            Tail::new(&watch)
                .with_debounce(Duration::from_millis(50))
                .with_sweep(Duration::from_mins(10))
                .run(db.conn(), |_| {}, &|| stop.load(Ordering::SeqCst))
                .expect("tail");
        })
    };

    let found = wait_until(Duration::from_secs(5), || {
        query::timeline_count(reader.conn(), &TimelineFilter::default()).unwrap_or(0) >= 1
    });

    stop.store(true, Ordering::SeqCst);
    std::fs::write(dir.path().join("nudge.jsonl"), "").ok();
    worker.join().expect("join");

    assert!(
        found,
        "a transcript already on disk was not read within five seconds, so the \
         first sweep is still waiting for its interval"
    );
}
