//! Phase 1.9 — storage measurement against the real corpus.
//!
//! Answers the question Phase 7 depends on: what does a tool call actually cost
//! on disk, and is inline storage of result bodies sustainable or does it need
//! storing oversized results by reference?
//!
//! Also settles Phase 1.5 by measuring external-content FTS5 against
//! contentless, rather than picking one on reputation.
//!
//! ```text
//! cargo run --release -p toolog-core --example measure_storage
//! ```
//!
//! **The extraction here is not the Phase 2 normalizer.** It pulls just enough
//! from each record to size the tables realistically — no per-tool normalizers,
//! no `structuredPatch`, no sidechain attribution. Phase 2 replaces it wholesale.
//!
//! Reads `~/.claude/projects` read-only and writes to a temporary database.

use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::Connection;
use serde_json::Value;
use toolog_core::db::Db;
use toolog_core::model::{Lane, NewRawEvent, Page, Session, TimelineFilter, TranscriptFacts};
use toolog_core::{project, query, raw};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let projects = home_projects_dir();
    if !projects.is_dir() {
        eprintln!("no transcripts at {}", projects.display());
        return Ok(());
    }

    let files = jsonl_files(&projects);
    let corpus_bytes: u64 = files
        .iter()
        .filter_map(|f| f.metadata().ok())
        .map(|m| m.len())
        .sum();
    println!("corpus: {} files, {}", files.len(), human(corpus_bytes));

    let tmp = tempfile::tempdir()?;
    let db_path = tmp.path().join("measure.db");
    let db = Db::open(&db_path)?;

    // --- ingest -------------------------------------------------------------
    let t0 = Instant::now();
    let mut lines = 0usize;
    let mut stored = 0usize;
    for file in &files {
        let text = std::fs::read_to_string(file)?;
        let name = file.to_string_lossy().to_string();
        let tx = db.conn().unchecked_transaction()?;
        let mut offset = 0i64;
        for line in text.lines() {
            lines += 1;
            let len = i64::try_from(line.len()).unwrap_or(i64::MAX) + 1;
            if !line.trim().is_empty() {
                let event = NewRawEvent {
                    lane: Lane::Transcript,
                    source_ref: &name,
                    source_offset: Some(offset),
                    body: line,
                };
                if raw::insert(&tx, &event)?.is_new() {
                    stored += 1;
                }
            }
            offset += len;
        }
        tx.commit()?;
    }
    let ingest = t0.elapsed();
    println!(
        "ingest: {lines} lines, {stored} stored ({} duplicates) in {:.1}s",
        lines - stored,
        ingest.as_secs_f64()
    );

    let pages_after_raw = page_bytes(db.conn())?;

    // --- project ------------------------------------------------------------
    let t1 = Instant::now();
    let mut p = SizingProjector::default();
    let stats = project::reproject(db.conn(), None, &mut p)?;
    let project_time = t1.elapsed();
    println!(
        "project: {} tool calls from {} records in {:.1}s ({} unparsed)",
        stats.tool_calls,
        stats.scanned,
        project_time.as_secs_f64(),
        p.skipped
    );

    db.conn().execute_batch("ANALYZE")?;
    report_storage(db.conn(), corpus_bytes, pages_after_raw, stats.tool_calls)?;
    compare_fts(db.conn())?;
    report_result_sizes(db.conn())?;
    report_latency(db.conn());
    report_shape(db.conn())?;

    println!(
        "\ndatabase file: {}",
        human(std::fs::metadata(&db_path)?.len())
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Report sections
// ---------------------------------------------------------------------------

/// Where the bytes went, and the headline per-1,000-calls figure.
fn report_storage(
    conn: &Connection,
    corpus_bytes: u64,
    raw_bytes: u64,
    tool_calls: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let total_bytes = page_bytes(conn)?;
    println!("\n=== storage ===");
    println!("{:<34} {:>12} {:>8}", "object", "bytes", "share");

    let mut rows = object_sizes(conn)?;
    rows.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
    for (name, bytes) in &rows {
        #[allow(clippy::cast_precision_loss)]
        let share = *bytes as f64 / total_bytes as f64 * 100.0;
        println!("{name:<34} {:>12} {share:>7.1}%", human(*bytes));
    }

    println!("{:<34} {:>12}", "TOTAL", human(total_bytes));
    println!(
        "  of which evidence (raw_event, ADR-0004): {} — the projection adds {}",
        human(raw_bytes),
        human(total_bytes.saturating_sub(raw_bytes))
    );
    #[allow(clippy::cast_precision_loss)]
    let amplification = total_bytes as f64 / corpus_bytes as f64;
    println!("database / corpus: {amplification:.2}x");

    if tool_calls > 0 {
        let per_1k = total_bytes * 1000 / u64::try_from(tool_calls).unwrap_or(1);
        println!("\n>>> {} per 1,000 tool calls <<<", human(per_1k));
    }
    Ok(())
}

/// Task 1.5: external-content FTS5 against contentless, on the real corpus.
fn compare_fts(conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== FTS5 comparison (task 1.5) ===");
    let external: u64 = object_sizes(conn)?
        .iter()
        .filter(|(n, _)| n.starts_with("tool_call_fts"))
        .map(|(_, b)| b)
        .sum();

    let before = page_bytes(conn)?;
    conn.execute_batch(
        "CREATE VIRTUAL TABLE probe_fts USING fts5(
             tool_name, input_summary, target_path, result_text,
             content='', tokenize='unicode61 remove_diacritics 2');
         INSERT INTO probe_fts (rowid, tool_name, input_summary, target_path, result_text)
             SELECT rowid, tool_name, input_summary, target_path, result_text FROM tool_call;",
    )?;
    let contentless = page_bytes(conn)? - before;
    conn.execute_batch("DROP TABLE probe_fts")?;

    println!("external-content: {:>12}", human(external));
    println!("contentless:      {:>12}", human(contentless));
    println!(
        "external-content also supports snippet()/highlight(), which Phase 5 needs\n\
         for match highlighting and contentless cannot provide."
    );
    Ok(())
}

/// Feeds the Phase 7 decision on storing oversized results by reference.
fn report_result_sizes(conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== result body sizes (drives the Phase 7 by-reference threshold) ===");
    for (label, sql) in [
        (
            "median",
            "SELECT result_size FROM tool_call WHERE result_size IS NOT NULL \
             ORDER BY result_size LIMIT 1 OFFSET (SELECT count(*)/2 FROM tool_call \
             WHERE result_size IS NOT NULL)",
        ),
        (
            "p95",
            "SELECT result_size FROM tool_call WHERE result_size IS NOT NULL \
             ORDER BY result_size LIMIT 1 OFFSET (SELECT count(*)*95/100 FROM tool_call \
             WHERE result_size IS NOT NULL)",
        ),
        ("max", "SELECT max(result_size) FROM tool_call"),
    ] {
        let v: Option<i64> = conn.query_row(sql, [], |r| r.get(0)).unwrap_or(None);
        println!(
            "{label:<8} {}",
            v.map_or("-".into(), |v| human(u64::try_from(v).unwrap_or(0)))
        );
    }

    let over_64k: i64 = conn.query_row(
        "SELECT count(*) FROM tool_call WHERE result_size > 65536",
        [],
        |r| r.get(0),
    )?;
    let sum_over: i64 = conn.query_row(
        "SELECT COALESCE(sum(result_size), 0) FROM tool_call WHERE result_size > 65536",
        [],
        |r| r.get(0),
    )?;
    println!(
        "{over_64k} calls have results over 64 KiB, totalling {}",
        human(u64::try_from(sum_over).unwrap_or(0))
    );
    Ok(())
}

/// Every query the Phase 5 UI will lean on.
fn report_latency(conn: &Connection) {
    println!("\n=== query latency ===");
    time("timeline_page (100)", || {
        query::timeline_page(conn, &TimelineFilter::default(), Page::default()).map(|v| v.len())
    });
    time("timeline_count (all)", || {
        query::timeline_count(conn, &TimelineFilter::default())
            .map(|v| usize::try_from(v).unwrap_or(0))
    });
    time("search 'cargo'", || {
        query::search(conn, "cargo", Page::default()).map(|v| v.len())
    });
    time("search 'rm -rf'", || {
        query::search(conn, "rm -rf", Page::default()).map(|v| v.len())
    });
    time("stats_tool_usage", || {
        query::stats_tool_usage(conn).map(|v| v.len())
    });
    time("reconcile", || {
        query::reconcile(conn).map(|r| usize::try_from(r.total()).unwrap_or(0))
    });
}

/// What the projection actually found, to sanity-check it against the corpus.
fn report_shape(conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== corpus shape ===");
    let totals = query::stats_totals(conn)?;
    println!(
        "sessions {} | tool calls {}",
        totals.sessions, totals.tool_calls
    );
    for u in query::stats_tool_usage(conn)?.iter().take(8) {
        println!("  {:<28} {:>6}", u.tool_name, u.calls);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Minimal sizing projector — NOT the Phase 2 normalizer.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct SizingProjector {
    skipped: usize,
}

impl project::Projector for SizingProjector {
    fn project(
        &mut self,
        conn: &Connection,
        event: &toolog_core::model::RawEvent,
    ) -> toolog_core::Result<()> {
        let Ok(v) = serde_json::from_str::<Value>(&event.body) else {
            self.skipped += 1;
            return Ok(());
        };

        let session_id = v.get("sessionId").and_then(Value::as_str).map(String::from);
        let ts = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_ts);

        if let Some(sid) = &session_id {
            project::upsert_session(
                conn,
                &Session {
                    session_id: sid.clone(),
                    project_path: v.get("cwd").and_then(Value::as_str).map(String::from),
                    transcript_path: Some(event.source_ref.clone()),
                    cwd: v.get("cwd").and_then(Value::as_str).map(String::from),
                    git_branch: v.get("gitBranch").and_then(Value::as_str).map(String::from),
                    cc_version: v.get("version").and_then(Value::as_str).map(String::from),
                    first_seen: ts,
                    last_seen: ts,
                    ..Session::default()
                },
            )?;
        }

        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let content = v.pointer("/message/content").and_then(Value::as_array);
                for block in content.into_iter().flatten() {
                    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    let (Some(id), Some(name)) = (
                        block.get("id").and_then(Value::as_str),
                        block.get("name").and_then(Value::as_str),
                    ) else {
                        continue;
                    };
                    let input = block.get("input");
                    let (server, tool) = split_mcp(name);
                    project::upsert_transcript(
                        conn,
                        id,
                        &TranscriptFacts {
                            session_id: session_id.clone(),
                            prompt_id: v.get("promptId").and_then(Value::as_str).map(String::from),
                            message_uuid: v.get("uuid").and_then(Value::as_str).map(String::from),
                            parent_uuid: v
                                .get("parentUuid")
                                .and_then(Value::as_str)
                                .map(String::from),
                            is_sidechain: v.get("isSidechain").and_then(Value::as_bool),
                            tool_name: Some(name.to_string()),
                            tool_kind: Some(
                                if server.is_some() { "mcp" } else { "builtin" }.into(),
                            ),
                            mcp_server: server,
                            mcp_tool: tool,
                            called_at: ts,
                            input_json: input.map(std::string::ToString::to_string),
                            input_summary: input.map(summarize_input),
                            target_path: input.and_then(target_of),
                            ..TranscriptFacts::default()
                        },
                    )?;
                }
            }
            Some("user") => {
                let Some(result) = v.get("toolUseResult") else {
                    return Ok(());
                };
                let content = v.pointer("/message/content").and_then(Value::as_array);
                let Some(id) = content
                    .into_iter()
                    .flatten()
                    .find_map(|b| b.get("tool_use_id").and_then(Value::as_str))
                else {
                    return Ok(());
                };
                let json = result.to_string();
                let text = result_text(result);
                project::upsert_transcript(
                    conn,
                    id,
                    &TranscriptFacts {
                        completed_at: ts,
                        result_size: Some(i64::try_from(json.len()).unwrap_or(i64::MAX)),
                        result_json: Some(json),
                        success: Some(!is_error(result)),
                        result_text: Some(text),
                        ..TranscriptFacts::default()
                    },
                )?;
            }
            _ => {}
        }
        Ok(())
    }
}

/// `mcp__server__tool` -> (server, tool).
fn split_mcp(name: &str) -> (Option<String>, Option<String>) {
    name.strip_prefix("mcp__").map_or((None, None), |rest| {
        rest.split_once("__")
            .map_or((Some(rest.into()), None), |(s, t)| {
                (Some(s.into()), Some(t.into()))
            })
    })
}

fn summarize_input(input: &Value) -> String {
    for key in [
        "command",
        "file_path",
        "url",
        "query",
        "pattern",
        "path",
        "prompt",
    ] {
        if let Some(s) = input.get(key).and_then(Value::as_str) {
            return s.chars().take(500).collect();
        }
    }
    input.to_string().chars().take(200).collect()
}

fn target_of(input: &Value) -> Option<String> {
    for key in ["file_path", "path", "url", "notebook_path"] {
        if let Some(s) = input.get(key).and_then(Value::as_str) {
            return Some(s.to_string());
        }
    }
    None
}

/// `toolUseResult` is an object, a bare string *or* an array — all three occur.
fn result_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(result_text).collect::<Vec<_>>().join("\n"),
        Value::Object(map) => {
            let mut out = String::new();
            for key in ["stdout", "stderr", "content", "text", "result", "codeText"] {
                if let Some(s) = map.get(key).and_then(Value::as_str) {
                    out.push_str(s);
                    out.push('\n');
                }
            }
            if out.is_empty()
                && let Some(f) = map
                    .get("file")
                    .and_then(|f| f.get("content"))
                    .and_then(Value::as_str)
            {
                out.push_str(f);
            }
            out
        }
        _ => String::new(),
    }
}

fn is_error(v: &Value) -> bool {
    v.get("interrupted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || v.get("is_error").and_then(Value::as_bool).unwrap_or(false)
        || v.get("stderr")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
}

fn parse_ts(s: &str) -> Option<i64> {
    s.parse::<jiff::Timestamp>()
        .ok()
        .map(jiff::Timestamp::as_millisecond)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn home_projects_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|d| d.home_dir().join(".claude").join("projects"))
        .unwrap_or_default()
}

fn jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn page_bytes(conn: &Connection) -> rusqlite::Result<u64> {
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok(u64::try_from(pages * size).unwrap_or(0))
}

fn object_sizes(conn: &Connection) -> rusqlite::Result<Vec<(String, u64)>> {
    let mut stmt =
        conn.prepare("SELECT name, sum(pgsize) FROM dbstat GROUP BY name ORDER BY 2 DESC")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
        ))
    })?;
    rows.collect()
}

fn time<T, E: std::fmt::Debug>(label: &str, mut f: impl FnMut() -> Result<T, E>)
where
    T: std::fmt::Display,
{
    let start = Instant::now();
    let out = f().expect("query");
    println!(
        "{label:<24} {:>8.2} ms   -> {out}",
        start.elapsed().as_secs_f64() * 1000.0
    );
}

fn human(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let b = bytes as f64;
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.1} KiB", b / 1024.0),
        1_048_576..=1_073_741_823 => format!("{:.1} MiB", b / 1_048_576.0),
        _ => format!("{:.2} GiB", b / 1_073_741_824.0),
    }
}
