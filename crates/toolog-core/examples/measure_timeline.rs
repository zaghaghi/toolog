//! Phase 5.3 — does the timeline's query surface hold at 100k rows?
//!
//! The phase sets two numbers: a first paint under 200 ms and a scroll that
//! stays smooth over the whole corpus. The frontend's half of that is
//! arithmetic — one row height, one division — so the half worth measuring is
//! this one: the count that sizes the scrollbar, the page that fills the
//! viewport, and the page that a jump to the middle of the list asks for.
//!
//! ```text
//! cargo run --release -p toolog-core --example measure_timeline
//! ```
//!
//! Writes a synthetic store to a temporary file and deletes it. Pass a path to
//! measure a real one instead, read-only:
//!
//! ```text
//! cargo run --release -p toolog-core --example measure_timeline -- ~/Library/Application\ Support/toolog/toolog.db
//! ```

use std::path::PathBuf;
use std::time::Instant;

use toolog_core::model::{OtelFacts, Page, Session, TimelineFilter, TranscriptFacts};
use toolog_core::{Connection, Db, project, query};

/// Rows in the synthetic store. The phase's stated target.
const ROWS: usize = 100_000;
const SESSIONS: usize = 400;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A path measures a real store, read-only. No path builds a synthetic one.
    if let Some(path) = std::env::args().nth(1) {
        let db = Db::open(PathBuf::from(&path))?;
        println!("measuring {path}\n");
        report(db.conn());
        return Ok(());
    }

    let path = std::env::temp_dir().join(format!("toolog-measure-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let db = Db::open(&path)?;
    println!("building {ROWS} calls across {SESSIONS} sessions…");
    let built = Instant::now();
    seed(db.conn())?;
    println!("built in {:?}\n", built.elapsed());
    report(db.conn());

    drop(db);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    Ok(())
}

/// A corpus shaped like the real one: mostly `Bash`, some subagents, and an
/// OTLP lane that has only seen the recent tail of it.
fn seed(conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    let tx = conn.unchecked_transaction()?;

    for s in 0..SESSIONS {
        project::upsert_session(
            conn,
            &Session {
                session_id: format!("session-{s:04}"),
                project_path: Some(format!("/Users/x/Projects/project-{}", s % 12)),
                git_branch: Some(if s % 3 == 0 {
                    "main".into()
                } else {
                    format!("feat/{s}")
                }),
                cc_version: Some("2.1.260".to_string()),
                ..Session::default()
            },
        )?;
    }

    // Roughly the observed mix: Bash 71%, Edit 10%, Read 9%, the rest spread.
    let tools = [
        "Bash", "Bash", "Bash", "Bash", "Bash", "Bash", "Bash", "Edit", "Read", "Write",
    ];

    for i in 0..ROWS {
        let n = i64::try_from(i).unwrap_or(i64::MAX);
        let session = i % SESSIONS;
        let sidechain = i % 9 == 0;
        let tool = tools[i % tools.len()];
        project::upsert_transcript(
            conn,
            &format!("toolu_{i:08}"),
            &TranscriptFacts {
                session_id: Some(format!("session-{session:04}")),
                tool_name: Some(tool.to_string()),
                tool_kind: Some("builtin".to_string()),
                input_summary: Some(format!(
                    "cargo test --workspace --all-targets -- --nocapture case_{i} \
                     && rm -rf target/debug/build-{i}"
                )),
                target_path: Some(format!(
                    "/Users/x/Projects/project-{}/src/mod_{i}.rs",
                    session % 12
                )),
                result_text: Some(format!("finished in {}ms; 0 failures; case {i}", i % 900)),
                called_at: Some(1_700_000_000_000 + n * 900),
                success: Some(i % 23 != 0),
                is_sidechain: Some(sidechain),
                agent_id: sidechain.then(|| format!("agent-{session}")),
                agent_name: sidechain.then(|| "Explore".to_string()),
                ..TranscriptFacts::default()
            },
        )?;

        // The OTLP lane has only witnessed the most recent tenth, which is what
        // an install partway through a corpus's life actually looks like.
        if i >= ROWS - ROWS / 10 {
            project::upsert_otel(
                conn,
                &format!("toolu_{i:08}"),
                &OtelFacts {
                    duration_ms: Some(n % 4000),
                    decision: Some(if i % 500 == 0 { "reject" } else { "accept" }.to_string()),
                    decision_source: Some(
                        if i % 7 == 0 {
                            "user_temporary"
                        } else {
                            "config"
                        }
                        .to_string(),
                    ),
                    permission_mode: Some("default".to_string()),
                    ..OtelFacts::default()
                },
            )?;
        }
    }

    tx.commit()?;
    conn.execute_batch("ANALYZE")?;
    Ok(())
}

fn report(conn: &Connection) {
    let all = TimelineFilter::default();
    let search = TimelineFilter {
        query: Some("rm -rf".to_string()),
        ..TimelineFilter::default()
    };
    let narrowed = TimelineFilter {
        query: Some("rm -rf".to_string()),
        project_path: Some(
            conn.query_row(
                "SELECT project_path FROM session WHERE project_path IS NOT NULL LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap_or_default(),
        ),
        ..TimelineFilter::default()
    };

    println!("{:<34}{:>10}{:>12}", "query", "rows", "time");
    println!("{}", "-".repeat(56));

    // First paint is these two together: the count sizes the scrollbar and the
    // first page fills the viewport.
    time("timeline_count (all)", || {
        query::timeline_count(conn, &all).map(|n| usize::try_from(n).unwrap_or(0))
    });
    time("timeline_rows (first 200)", || {
        query::timeline_rows(
            conn,
            &all,
            Page {
                limit: 200,
                offset: 0,
            },
        )
        .map(|v| v.len())
    });
    time("timeline_rows (offset 90k)", || {
        query::timeline_rows(
            conn,
            &all,
            Page {
                limit: 200,
                offset: 90_000,
            },
        )
        .map(|v| v.len())
    });
    time("timeline_groups (all)", || {
        query::timeline_groups(conn, &all).map(|v| v.len())
    });
    time("search 'rm -rf' count", || {
        query::timeline_count(conn, &search).map(|n| usize::try_from(n).unwrap_or(0))
    });
    time("search 'rm -rf' first page", || {
        query::timeline_rows(
            conn,
            &search,
            Page {
                limit: 200,
                offset: 0,
            },
        )
        .map(|v| v.len())
    });
    time("search + project filter", || {
        query::timeline_rows(
            conn,
            &narrowed,
            Page {
                limit: 200,
                offset: 0,
            },
        )
        .map(|v| v.len())
    });

    report_search(conn);
}

/// The search half, which is where the ordering cost lives.
fn report_search(conn: &Connection) {
    // A broad but not universal term: the prefix wildcard makes this match
    // about one row in nine, which is the shape of a real broad search.
    let broad = TimelineFilter {
        query: Some("case_1".to_string()),
        ..TimelineFilter::default()
    };
    time("search (1 row in 9) count", || {
        query::timeline_count(conn, &broad).map(|n| usize::try_from(n).unwrap_or(0))
    });
    time("search (1 row in 9) page", || {
        query::timeline_rows(
            conn,
            &broad,
            Page {
                limit: 200,
                offset: 0,
            },
        )
        .map(|v| v.len())
    });

    // The realistic shape of a search: a term that matches a handful of rows
    // rather than every one of them.
    let selective = TimelineFilter {
        query: Some("case_99321".to_string()),
        ..TimelineFilter::default()
    };
    time("search (selective term)", || {
        query::timeline_rows(
            conn,
            &selective,
            Page {
                limit: 200,
                offset: 0,
            },
        )
        .map(|v| v.len())
    });
    time("facets", || query::facets(conn).map(|f| f.tools.len()));
}

fn time<E: std::fmt::Debug>(label: &str, f: impl Fn() -> Result<usize, E>) {
    // Warm once so the figure is the query, not the first page fault.
    let _ = f();
    let started = Instant::now();
    let n = f().expect("query");
    println!("{label:<34}{n:>10}{:>12?}", started.elapsed());
}
