//! What the CLI subcommands actually do.
//!
//! Kept apart from argument parsing so the Tauri commands can call the same
//! functions: "Run Backfill" in the tray and `toolog backfill` on the command
//! line must not be two implementations that drift.

use std::io::Write;
use std::path::{Path, PathBuf};

use toolog_core::analytics;
use toolog_core::model::{Page, TimelineFilter, ToolCall};
use toolog_core::{Connection, Db, Result, project, query};
use toolog_ingest::Backfill;

/// Import existing history.
///
/// Safe to re-run: content-hash deduplication makes a repeat pass a no-op, so
/// this doubles as "catch up on whatever happened while nothing was watching".
pub fn backfill(db: &Db, root: Option<&Path>, progress: impl Fn(&str)) -> Result<Summary> {
    let run = Backfill::new(db.conn()).on_progress(|report| {
        progress(&format!(
            "{}: {} lines, {} new",
            report.path.display(),
            report.lines,
            report.stored
        ));
    });

    let report = match root {
        Some(root) => run.run(root)?,
        None => run.run_default()?,
    };

    Ok(Summary {
        files: report.files,
        lines: report.lines,
        stored: report.stored,
        duplicates: report.duplicates,
        tool_uses: report.stats.tool_uses,
        sessions: report.stats.sessions,
    })
}

/// What a backfill did, flattened for display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Summary {
    pub files: usize,
    pub lines: usize,
    pub stored: usize,
    pub duplicates: usize,
    pub tool_uses: usize,
    pub sessions: usize,
}

/// Rebuild every projection table from `raw_event`, across **both** lanes.
///
/// The escape hatch [ADR-0004] exists for: when normalization changes or a past
/// parser missed a field, the projections are derived again from the evidence,
/// which was stored verbatim precisely so this is possible.
///
/// It must be given both projectors. Re-projecting with one lane's projector
/// clears the other lane's columns — a `toolog backfill` did exactly that
/// before this function existed, silently deleting every permission decision
/// and duration in the store.
///
/// [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md
pub fn reproject_all(conn: &Connection) -> Result<project::ReprojectStats> {
    let mut transcript = toolog_ingest::TranscriptProjector::new();
    let mut otlp = toolog_otlp::OtlpProjector::new();
    let mut both = project::Chain::new(vec![&mut transcript, &mut otlp]);
    project::reproject(conn, None, &mut both)
}

/// Cross-check the two ingestion lanes ([ADR-0009]).
///
/// The full per-session completeness report is Phase 7; this is the totals,
/// which is already enough to answer "did anything get refused, and is
/// anything missing?".
///
/// [ADR-0009]: ../../../docs/adr/0009-correlate-on-tool-use-id.md
pub fn verify(db: &Db) -> Result<toolog_core::verify::Completeness> {
    toolog_core::verify::completeness(db.conn())
}

/// Render a reconciliation the way an auditor reads it.
#[must_use]
pub fn render_verify(c: &toolog_core::verify::Completeness) -> String {
    use std::fmt::Write as _;
    let r = &c.lanes;
    let mut out = String::new();

    let _ = writeln!(out, "Lane reconciliation");
    let _ = writeln!(
        out,
        "  both lanes      {:>8}   content and decision, the complete record",
        r.both
    );
    let _ = writeln!(
        out,
        "  transcript only {:>8}   what ran, but not who approved it",
        r.transcript_only
    );
    let _ = writeln!(
        out,
        "  OTEL only       {:>8}   no transcript body was written for these",
        r.otel_only
    );
    let _ = writeln!(
        out,
        "\n  refused         {:>8}   calls a permission rule or a person denied",
        r.rejected
    );

    if let Some(ratio) = c.decided_ratio() {
        let _ = writeln!(
            out,
            "\n  {:.1}% of {} calls have their approval on record.",
            ratio * 100.0,
            r.total()
        );
    }

    out.push_str(&render_gaps(c));
    out.push_str(&render_sessions(c));

    let _ = writeln!(
        out,
        "\nTranscript-only calls are expected for history imported from before toolog ran.\n\
         Refusals are counted from the decision the OTLP lane carries, not from a missing\n\
         transcript: a denied call does leave a transcript record, but nothing in it says\n\
         who denied it or why."
    );
    out
}

/// The windows in which nothing was watching.
fn render_gaps(c: &toolog_core::verify::Completeness) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if c.gaps.is_empty() {
        return out;
    }

    let _ = writeln!(
        out,
        "\nWindows with no decision layer ({}, largest first)",
        plural(i64::try_from(c.gaps.len()).unwrap_or(i64::MAX), "window"),
    );
    for gap in &c.gaps {
        let _ = writeln!(
            out,
            "  {} → {}   {:>6} {:<8} in {:<12} ({})",
            stamp(Some(gap.from_ms)),
            stamp(Some(gap.to_ms)),
            gap.calls,
            if gap.calls == 1 { "call" } else { "calls" },
            plural(gap.sessions, "session"),
            span(gap.duration_ms()),
        );
    }
    out
}

/// Sessions whose approval layer is incomplete, worst first.
fn render_sessions(c: &toolog_core::verify::Completeness) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let incomplete: Vec<_> = c.sessions.iter().filter(|s| s.decided < s.calls).collect();
    if incomplete.is_empty() {
        return out;
    }

    let shown = incomplete.len();
    let _ = writeln!(
        out,
        "\nSessions missing part of their approval record — {} of {}{}, least complete first",
        c.sessions_incomplete,
        plural(c.sessions_total, "session"),
        if i64::try_from(shown).unwrap_or(i64::MAX) < c.sessions_incomplete {
            format!(" ({shown} shown)")
        } else {
            String::new()
        },
    );
    for session in incomplete {
        let _ = writeln!(
            out,
            "  {:<38} {:>5}/{:<5} {:>6}   {}",
            session.session_id,
            session.decided,
            session.calls,
            session
                .decided_ratio()
                .map_or_else(|| "—".to_string(), |r| format!("{:.0}%", r * 100.0)),
            session
                .project_path
                .as_deref()
                .unwrap_or("(unknown project)"),
        );
    }
    out
}

/// `1 call` / `2 calls`, for the many places one number needs a noun.
fn plural(n: i64, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// A duration in the words a gap is described in.
fn span(ms: i64) -> String {
    let minutes = ms / 60_000;
    if minutes < 1 {
        return "under a minute".to_string();
    }
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h {:02}m", minutes % 60);
    }
    format!("{}d {:02}h", hours / 24, hours % 24)
}

/// Walk the integrity chain and report it (task 7.6).
pub fn verify_chain(db: &Db) -> Result<toolog_core::chain::ChainReport> {
    toolog_core::chain::verify(db.conn())
}

/// Render a chain report, ending with the head worth recording elsewhere.
#[must_use]
pub fn render_chain(report: &toolog_core::chain::ChainReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    let _ = writeln!(out, "\nIntegrity chain");
    let _ = writeln!(out, "  records checked {:>8}", report.checked);
    if report.unsealed > 0 {
        let _ = writeln!(
            out,
            "  not yet sealed  {:>8}   stored before this version; run a backfill or start \
             the app to seal them",
            report.unsealed
        );
    }

    let unexplained = report.unexplained().len();
    if report.checked == 0 {
        // "Intact" over nothing is true and misleading.
        let _ = writeln!(
            out,
            "  intact          {:>8}   nothing is sealed yet",
            "n/a"
        );
    } else if report.intact() {
        let _ = writeln!(out, "  intact          {:>8}", "yes");
    } else if unexplained == 0 {
        // A store that bounds itself breaks its own chain on purpose. Calling
        // that tampering would have this check switched off within a week.
        let _ = writeln!(
            out,
            "  intact          {:>8}   {}, {} this store recorded",
            "accounted for",
            plural(
                i64::try_from(report.breaks.len()).unwrap_or(i64::MAX),
                "break"
            ),
            if report.breaks.len() == 1 {
                "a hole"
            } else {
                "all of them holes"
            },
        );
    } else {
        let _ = writeln!(out, "  intact          {:>8}", "NO");
    }

    // Every break, not a summary: one is an edited row, a hundred is a chain
    // rewritten from some point onward, and the difference matters.
    for b in report.breaks.iter().take(20) {
        match &b.explained_by {
            Some(why) => {
                let _ = writeln!(out, "    raw_event {:<10} {} — {why}", b.id, b.what);
            }
            None => {
                let _ = writeln!(out, "    raw_event {:<10} {}  ← unexplained", b.id, b.what);
            }
        }
    }
    if report.breaks.len() > 20 {
        let _ = writeln!(out, "    … and {} more", report.breaks.len() - 20);
    }

    let _ = writeln!(out, "\n  head  {}", report.head);
    let _ = writeln!(
        out,
        "\nThe head covers every record before it. Keep it somewhere outside this database —\n\
         a note, a commit, a message to yourself — and a later run that reports a different\n\
         head for the same records is the only way to catch a chain that was rewritten\n\
         wholesale. Walking detects everything short of that."
    );

    let purges = deletions_line(report);
    if !purges.is_empty() {
        let _ = write!(out, "{purges}");
    }
    out
}

/// The purges a chain report's breaks were matched against.
fn deletions_line(report: &toolog_core::chain::ChainReport) -> String {
    use std::fmt::Write as _;
    let explained = report
        .breaks
        .iter()
        .filter(|b| b.explained_by.is_some())
        .count();
    if explained == 0 {
        return String::new();
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\n{} above {} a hole left by `toolog purge`, matched to the record that purge\n\
         wrote. A hole nothing accounts for is the one to look at.",
        plural(i64::try_from(explained).unwrap_or(i64::MAX), "break"),
        if explained == 1 { "is" } else { "are" },
    );
    out
}

/// Output shapes for `toolog export`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    ts_rs::TS,
)]
#[serde(rename_all = "lowercase")]
#[ts(export_to = "unused/")]
pub enum Format {
    /// One JSON array. Convenient for `jq`.
    Json,
    /// One JSON object per line. Streams, and survives being truncated.
    Jsonl,
    /// Spreadsheet columns: the display fields only, not the full payloads.
    Csv,
    /// A table to paste into a report or an issue. Human-readable, and the
    /// only format that spells out which lanes witnessed each row.
    Markdown,
}

/// Which lanes witnessed a row, in words.
///
/// An export is evidence, and evidence that does not say how completely it was
/// observed is worth less than one that does. `transcript` means no decision,
/// duration or cost was ever recorded for that call.
#[must_use]
fn lanes(provenance: i64) -> &'static str {
    match (
        provenance & toolog_core::model::provenance::TRANSCRIPT != 0,
        provenance & toolog_core::model::provenance::OTLP != 0,
    ) {
        (true, true) => "both",
        (true, false) => "transcript",
        (false, true) => "otel",
        (false, false) => "none",
    }
}

/// An epoch-millisecond timestamp as RFC 3339 UTC, for a human-readable export.
fn stamp(ms: Option<i64>) -> String {
    ms.and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
        .map(|t| t.to_string())
        .unwrap_or_default()
}

/// One Markdown table cell.
///
/// Shell commands contain pipes and newlines, both of which end a table cell.
fn md(value: Option<&str>) -> String {
    value
        .unwrap_or("")
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace(['\n', '\r'], " ")
}

/// Write one row in the chosen format.
///
/// `written` is how many rows are already out, which JSON needs for its commas.
fn write_row(out: &mut dyn Write, row: &ToolCall, format: Format, written: u32) -> Result<()> {
    match format {
        Format::Json => {
            if written > 0 {
                writeln!(out, ",")?;
            }
            write!(out, "{}", serde_json::to_string(row).unwrap_or_default())?;
        }
        Format::Jsonl => {
            writeln!(out, "{}", serde_json::to_string(row).unwrap_or_default())?;
        }
        Format::Csv => {
            writeln!(
                out,
                "{},{},{},{},{},{},{},{},{},{}",
                row.called_at.map(|t| t.to_string()).unwrap_or_default(),
                csv(row.session_id.as_deref()),
                csv(row.tool_name.as_deref()),
                row.success.map(|s| s.to_string()).unwrap_or_default(),
                row.duration_ms.map(|d| d.to_string()).unwrap_or_default(),
                csv(row.decision.as_deref()),
                csv(row.decision_source.as_deref()),
                csv(Some(lanes(row.provenance))),
                csv(row.target_path.as_deref()),
                csv(row.input_summary.as_deref()),
            )?;
        }
        Format::Markdown => {
            writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} |",
                stamp(row.called_at),
                md(row.tool_name.as_deref()),
                match (row.decision.as_deref(), row.success) {
                    (Some("reject"), _) => "refused".to_string(),
                    (_, Some(true)) => "ok".to_string(),
                    (_, Some(false)) => "failed".to_string(),
                    (_, None) => String::new(),
                },
                row.duration_ms
                    .map(|d| format!("{d} ms"))
                    .unwrap_or_default(),
                md(row.decision_source.as_deref()),
                lanes(row.provenance),
                md(row.input_summary.as_deref().or(row.target_path.as_deref())),
            )?;
        }
    }
    Ok(())
}

/// Write matching tool calls to `out`.
///
/// Takes a connection rather than a [`Db`] so the application can export
/// through its existing read handle instead of opening a second one.
pub fn export(
    conn: &Connection,
    filter: &TimelineFilter,
    limit: Option<u32>,
    format: Format,
    out: &mut dyn Write,
) -> Result<u32> {
    // Paged rather than one query so an export of a large store does not build
    // the whole result set in memory before writing a byte.
    const PAGE: u32 = 500;

    let mut written = 0u32;
    let mut offset = 0u32;

    if format == Format::Json {
        writeln!(out, "[")?;
    }
    if format == Format::Csv {
        writeln!(
            out,
            "called_at,session_id,tool_name,success,duration_ms,decision,decision_source,lanes,target_path,input_summary"
        )?;
    }
    if format == Format::Markdown {
        writeln!(out, "# toolog export\n")?;
        writeln!(out, "Exported {}.\n", jiff::Timestamp::now())?;
        writeln!(
            out,
            "| Time (UTC) | Tool | Result | Duration | Decision | Lanes | Summary |"
        )?;
        writeln!(out, "|---|---|---|---|---|---|---|")?;
    }

    loop {
        let remaining = limit.map_or(PAGE, |l| PAGE.min(l.saturating_sub(written)));
        if remaining == 0 {
            break;
        }
        let page = Page {
            limit: remaining,
            offset,
        };
        let rows = query::timeline_page(conn, filter, page)?;
        if rows.is_empty() {
            break;
        }

        for row in &rows {
            write_row(out, row, format, written)?;
            written += 1;
        }

        offset += u32::try_from(rows.len()).unwrap_or(PAGE);
    }

    if format == Format::Json {
        writeln!(out, "\n]")?;
    }
    if format == Format::Markdown {
        writeln!(
            out,
            "\n{written} calls. **Lanes** is how completely each call was observed: \
             `both` is the full record; `transcript` means no decision, duration or cost \
             was ever recorded for it; `otel` means no transcript body was written."
        )?;
    }
    out.flush()?;
    Ok(written)
}

/// Quote a CSV field, doubling any embedded quotes.
///
/// Shell commands routinely contain commas, quotes and newlines; an unquoted
/// export of them is not evidence, it is a corrupted file.
fn csv(value: Option<&str>) -> String {
    let value = value.unwrap_or_default();
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// Where a user's own rules live, if they have written any.
#[must_use]
pub fn rules_path() -> Option<PathBuf> {
    toolog_core::db::default_path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("rules.toml")))
}

/// Every rule in force: the built-in set, plus the user's file if it exists.
pub fn rules() -> Result<Vec<toolog_core::rules::Rule>> {
    let user = rules_path().and_then(|p| std::fs::read_to_string(p).ok());
    toolog_core::rules::load(user.as_deref())
}

/// Evaluate the risk rules against the store.
pub fn risk(db: &Db) -> Result<Vec<toolog_core::rules::Finding>> {
    toolog_core::rules::evaluate(db.conn(), &rules()?)
}

/// Render findings the way a review is read: worst first, with examples.
#[must_use]
pub fn render_risk(findings: &[toolog_core::rules::Finding]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    if findings.is_empty() {
        return "No rule matched anything in this store.\n".to_string();
    }

    for f in findings {
        let severity = format!("{:?}", f.severity).to_uppercase();
        let _ = writeln!(out, "[{severity}] {}", f.title);
        let _ = writeln!(
            out,
            "  {} {} across {} session{}{}",
            f.calls,
            if f.calls == 1 { "call" } else { "calls" },
            f.sessions,
            if f.sessions == 1 { "" } else { "s" },
            if f.projects.is_empty() {
                String::new()
            } else {
                format!(" in {}", f.projects.len())
            },
        );
        if let Some(d) = &f.dismissed {
            let _ = writeln!(out, "  dismissed: {}", d.note);
        }
        for call in f.examples.iter().take(3) {
            let summary = call
                .input_summary
                .as_deref()
                .or(call.target_path.as_deref())
                .unwrap_or("");
            let summary: String = summary.chars().take(90).collect();
            let _ = writeln!(out, "    {} {summary}", call.tool_use_id);
        }
        let _ = writeln!(out);
    }
    out
}

// ---------------------------------------------------------------------------
// Usage (tasks 6.5-6.8)
// ---------------------------------------------------------------------------

/// Everything `toolog usage` reports, in one pass over one window.
#[derive(Debug)]
pub struct UsageReport {
    pub analytics: analytics::Analytics,
    pub comparison: analytics::Comparison,
}

/// Resolve `--days` and `--project` into a window.
///
/// Days back from now rather than calendar days, and absolute once resolved,
/// for the same reason the UI's presets are: a window that means something
/// different tomorrow is not a window a report can be checked against.
#[must_use]
pub fn usage_window(days: Option<u32>, project: Option<String>) -> analytics::Period {
    let now = jiff::Zoned::now();
    let since = days.map(|d| now.timestamp().as_millisecond() - i64::from(d) * 86_400_000);
    analytics::Period {
        since,
        until: since.map(|_| now.timestamp().as_millisecond()),
        project_path: project,
        utc_offset_minutes: now.offset().seconds() / 60,
    }
}

/// Compute the report.
pub fn usage(db: &Db, window: &analytics::Period) -> Result<UsageReport> {
    Ok(UsageReport {
        analytics: analytics::analytics(db.conn(), window)?,
        comparison: analytics::compare(db.conn(), window)?,
    })
}

/// A cost in micro-dollars.
///
/// Sub-cent spend was still *measured*, so it must not render as `$0.00`: the
/// only thing in this report that means "not measured" is the words.
fn money(micros: i64) -> String {
    if micros > 0 && micros < 10_000 {
        return "<$0.01".to_string();
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "micro-dollars for display; a 2^53 total would be $9bn"
    )]
    let dollars = micros as f64 / 1_000_000.0;
    format!("${dollars:.2}")
}

/// A ratio as a percentage, or a dash when nothing was measured.
fn pct(ratio: Option<f64>) -> String {
    ratio.map_or_else(|| "—".to_string(), |r| format!("{:.1}%", r * 100.0))
}

/// A duration, or a dash when the OTLP lane never timed these calls.
fn ms(value: Option<i64>) -> String {
    value.map_or_else(|| "—".to_string(), |v| format!("{v} ms"))
}

/// Render the report the way it is read: headline first, then where it went.
///
/// Cost has three states here, not two. `not captured` is not `$0.00`, and the
/// coverage line says which of the two this store is in (task 6.8).
#[must_use]
pub fn render_usage(report: &UsageReport) -> String {
    use std::fmt::Write as _;
    let a = &report.analytics;
    let mut out = String::new();

    if a.calls.calls == 0 {
        return "No calls in this window.\n".to_string();
    }

    let mins = |ms: i64| {
        let minutes = ms / 60_000;
        if minutes < 60 {
            format!("{minutes}m")
        } else {
            format!("{}h {:02}m", minutes / 60, minutes % 60)
        }
    };

    let _ = writeln!(
        out,
        "{} in {} across {}, {} active",
        plural(a.calls.calls, "call"),
        plural(a.calls.sessions, "session"),
        plural(a.calls.projects, "project"),
        mins(a.calls.active_ms),
    );
    let _ = writeln!(
        out,
        "  {} failed of {} with a recorded outcome ({}), {} refused",
        a.calls.failures,
        a.calls.with_outcome,
        pct(a.calls.error_rate),
        a.calls.refused,
    );
    let _ = writeln!(
        out,
        "  p50 {}, p95 {}, {} from subagents",
        ms(a.calls.p50_ms),
        ms(a.calls.p95_ms),
        pct(a.calls.sidechain_share),
    );

    if a.coverage.measured {
        let _ = writeln!(
            out,
            "\n{} over {} requests, {} tokens, {} from cache",
            money(a.cost.cost_usd_micros),
            a.cost.requests,
            a.cost.total_tokens,
            pct(a.cost.cache_hit_ratio),
        );
    } else {
        let _ = writeln!(out, "\nNo cost captured.");
    }
    if !a.coverage.complete {
        let _ = writeln!(
            out,
            "  Cost covers {} of {} sessions ({} of {} calls). The rest were \
             imported from transcripts, which record no cost.",
            a.coverage.sessions_with_cost,
            a.coverage.sessions,
            a.coverage.calls_with_cost,
            a.coverage.calls,
        );
    }

    if let (Some(previous), Some(window)) = (
        &report.comparison.previous,
        &report.comparison.previous_window,
    ) {
        let _ = writeln!(
            out,
            "\nAgainst the period before ({}, {}, {}):",
            plural(previous.calls, "call"),
            plural(previous.sessions, "session"),
            if previous.sessions_with_cost > 0 {
                money(previous.cost_usd_micros)
            } else {
                "no cost captured".to_string()
            },
        );
        let _ = writeln!(
            out,
            "  {} to {}",
            stamp(window.since),
            stamp(report.comparison.current_window.since),
        );
    }

    out.push_str(&render_breakdowns(a));
    out
}

/// The three "where it went" tables.
fn render_breakdowns(a: &analytics::Analytics) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    let _ = writeln!(out, "\nBy project");
    for bucket in a.by_project.iter().take(10) {
        let _ = writeln!(
            out,
            "  {:<40} {:>6} calls  {:>12}",
            bucket.key.as_deref().unwrap_or("(unknown)"),
            bucket.calls,
            if bucket.requests > 0 {
                money(bucket.cost_usd_micros)
            } else {
                "not captured".to_string()
            },
        );
    }

    if !a.by_model.is_empty() {
        let _ = writeln!(out, "\nBy model");
        for bucket in &a.by_model {
            let _ = writeln!(
                out,
                "  {:<40} {:>6} reqs   {:>12}",
                bucket.key.as_deref().unwrap_or("(unknown)"),
                bucket.requests,
                money(bucket.cost_usd_micros),
            );
        }
    }

    let _ = writeln!(out, "\nBy tool");
    for tool in a.tools.iter().take(10) {
        let _ = writeln!(
            out,
            "  {:<40} {:>6} calls  {:>4} failed  p50 {}",
            tool.tool_name,
            tool.calls,
            tool.failures,
            ms(tool.p50_ms),
        );
    }

    out
}

// ---------------------------------------------------------------------------
// Retention (tasks 7.4 and 7.8)
// ---------------------------------------------------------------------------

/// A size in the units a person reads.
#[must_use]
pub fn bytes(n: i64) -> String {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a size for display; a store this large would be terabytes"
    )]
    let value = n as f64;
    if n < 1024 {
        return format!("{n} B");
    }
    if n < 1024 * 1024 {
        return format!("{:.1} KiB", value / 1024.0);
    }
    if n < 1024 * 1024 * 1024 {
        return format!("{:.1} MiB", value / (1024.0 * 1024.0));
    }
    format!("{:.2} GiB", value / (1024.0 * 1024.0 * 1024.0))
}

/// Show exactly what a purge would remove, before it removes it.
///
/// The whole point of task 7.4: not a count, but the sessions themselves, so
/// the answer to "is this the right thing to delete?" is on the screen rather
/// than inferred from a number.
#[must_use]
pub fn render_purge(plan: &toolog_core::retention::Preview, applying: bool) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    if plan.is_empty() {
        let _ = writeln!(out, "Nothing to remove: {}.", plan.description);
        return out;
    }

    let _ = writeln!(
        out,
        "{} would remove {} of {} sessions, {} of {} tool calls, and {} stored records ({}).",
        if applying {
            "This"
        } else {
            "This is a preview. It"
        },
        plan.sessions.len(),
        plan.total_sessions,
        plan.tool_calls,
        plan.total_tool_calls,
        plan.raw_events,
        bytes(plan.bytes),
    );
    if plan.otlp_records > 0 {
        let _ = writeln!(
            out,
            "  {} of those are OTLP records, which belong to no transcript and go by the cutoff.",
            plan.otlp_records
        );
    }

    let _ = writeln!(out, "\nSessions, oldest first");
    for session in &plan.sessions {
        let _ = writeln!(
            out,
            "  {:<38} {:>6} calls  {:>10}  {}  {}",
            session.session_id,
            session.tool_calls,
            bytes(session.bytes),
            stamp(session.last_seen),
            session
                .project_path
                .as_deref()
                .unwrap_or("(unknown project)"),
        );
    }

    if !applying {
        let _ = writeln!(
            out,
            "\nNothing has been deleted. Run the same command with --apply to remove it."
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Uninstall (task 8.6)
// ---------------------------------------------------------------------------

/// Show exactly what an uninstall would do, before it does it.
///
/// Three things earn their space here. **Which way the settings file goes
/// back** — a byte-identical restore or a key removal — because those have
/// different consequences and the user is the one who knows which is right.
/// **That history is kept by default**, said before it can be assumed
/// otherwise. And **the `.app` itself**, which this process cannot delete while
/// running inside it, so pretending otherwise would leave the job half done
/// with no sign of it.
#[must_use]
pub fn render_uninstall(plan: &crate::uninstall::Plan, applying: bool) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}\n",
        if applying {
            "Uninstalling toolog."
        } else {
            "This is a preview. Nothing below has happened yet."
        }
    );

    let _ = writeln!(out, "Login agent");
    match &plan.agent {
        Some(path) => {
            let _ = writeln!(out, "  remove   {}", path.display());
            let _ = writeln!(out, "           capture will not start at login again");
        }
        None => {
            let _ = writeln!(out, "  --       not installed");
        }
    }

    out.push_str(&render_uninstall_settings(plan));
    out.push_str(&render_uninstall_data(plan));
    out.push_str(&render_uninstall_footer(plan, applying));
    out
}

/// How `~/.claude/settings.json` goes back, and why that way.
fn render_uninstall_settings(plan: &crate::uninstall::Plan) -> String {
    use crate::uninstall::SettingsRevert;
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "\nClaude Code configuration");
    let _ = writeln!(out, "  file:    {}", plan.settings_path.display());
    match &plan.settings {
        SettingsRevert::Clean => {
            let _ = writeln!(out, "  --       none of our keys are in it");
        }
        SettingsRevert::RestoreBackup { backup, keys } => {
            let _ = writeln!(
                out,
                "  restore  from {}\n           {} keys go, and the file returns byte for byte to \
                 what it was\n           before toolog first wrote to it",
                backup.display(),
                keys.len()
            );
        }
        SettingsRevert::RemoveKeys { keys, reason } => {
            let _ = writeln!(
                out,
                "  edit     remove {} keys, keep everything else",
                keys.len()
            );
            let _ = writeln!(out, "           not a byte-identical restore, because");
            let _ = writeln!(out, "           {reason}");
        }
        SettingsRevert::RemoveFile { keys } => {
            let _ = writeln!(
                out,
                "  delete   the file holds nothing but our {} keys, and did not exist\n           \
                 before the install",
                keys.len()
            );
        }
        SettingsRevert::BeyondOurReach { scopes } => {
            let _ = writeln!(
                out,
                "  --       set by {}, which toolog never writes and will not edit",
                scopes
                    .iter()
                    .map(|s| s.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        SettingsRevert::Unreadable { message } => {
            let _ = writeln!(out, "  !        {message}; not editing it");
        }
    }
    out
}

/// What happens to the record itself. Kept unless asked otherwise.
fn render_uninstall_data(plan: &crate::uninstall::Plan) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "\nYour recorded history");
    if plan.data.is_empty() {
        let _ = writeln!(out, "  --       nothing stored");
    } else if plan.delete_data {
        for item in &plan.data {
            let _ = writeln!(
                out,
                "  delete   {:>10}  {}\n           {}",
                bytes(i64::try_from(item.bytes).unwrap_or(i64::MAX)),
                item.path.display(),
                item.what
            );
        }
        let _ = writeln!(
            out,
            "\n  {} in total, and it is not recoverable.",
            bytes(i64::try_from(plan.data_bytes()).unwrap_or(i64::MAX))
        );
    } else {
        let _ = writeln!(
            out,
            "  keep     {} in {}",
            bytes(i64::try_from(plan.data_bytes()).unwrap_or(i64::MAX)),
            plan.data_dir.as_deref().map_or_else(
                || "the data directory".to_string(),
                |d| d.display().to_string()
            )
        );
        let _ = writeln!(
            out,
            "           Kept on purpose: an audit trail outlives the tool that\n           \
             collected it. Pass --delete-data to remove it as well."
        );
    }

    out
}

/// The two things this command cannot do for you, and the way out.
fn render_uninstall_footer(plan: &crate::uninstall::Plan, applying: bool) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    if plan.running {
        let _ = writeln!(
            out,
            "\nNote: toolog is running. Quit it from the menu bar afterwards, or the\n\
             receiver stays up until you log out."
        );
    }
    if let Some(app) = &plan.app_bundle {
        let _ = writeln!(
            out,
            "\nThe application itself is not removed by this command — a running app\n\
             cannot delete itself. Afterwards:\n\
             \x20 brew uninstall --cask toolog\n\
             or move {} to the Trash.",
            app.display()
        );
    }

    if !applying {
        let _ = writeln!(
            out,
            "\nNothing has changed. Run the same command with --apply to do it."
        );
    }
    out
}

/// A default file name for an export: `toolog-2026-09-05`.
///
/// Dated rather than timestamped: an evidence bundle is usually re-exported a
/// few times in one sitting, and a name that collides is a name the save panel
/// will warn about, which is the right prompt.
#[must_use]
pub fn export_file_stem() -> String {
    jiff::Zoned::now().strftime("toolog-%Y-%m-%d").to_string()
}

/// Only calls that were refused.
///
/// Read from `decision`, which the OTLP lane supplies. **Not** inferred from
/// provenance: Phase 4 measured two real refusals and found both in *both*
/// lanes, because a denied call still leaves a `tool_use` block and a
/// `tool_result` whose body is the refusal message. The lane-based version of
/// this filter matched every call OTEL witnessed — 30 rows on the owner's
/// store, of which 2 were actually refused.
#[must_use]
pub fn rejected_only() -> TimelineFilter {
    TimelineFilter {
        decision: Some("reject".to_string()),
        ..TimelineFilter::default()
    }
}

/// The default database location, for callers that do not specify one.
pub fn default_db_path() -> Result<PathBuf> {
    toolog_core::db::default_path()
}

#[cfg(test)]
mod tests {
    use super::*;
    use toolog_core::model::{Lane, NewRawEvent, TranscriptFacts};
    use toolog_core::{project, raw};

    fn seeded() -> Db {
        let db = Db::open_in_memory().expect("db");
        raw::insert(
            db.conn(),
            &NewRawEvent {
                lane: Lane::Transcript,
                source_ref: "t.jsonl",
                source_offset: Some(0),
                body: "{}",
            },
        )
        .expect("raw");
        project::upsert_transcript(
            db.conn(),
            "toolu_1",
            &TranscriptFacts {
                session_id: Some("s1".to_string()),
                tool_name: Some("Bash".to_string()),
                called_at: Some(1),
                input_summary: Some("echo \"hi, there\"".to_string()),
                ..TranscriptFacts::default()
            },
        )
        .expect("project");
        db
    }

    #[test]
    fn csv_survives_commands_containing_commas_and_quotes() {
        let db = seeded();
        let mut out = Vec::new();
        let n = export(
            db.conn(),
            &TimelineFilter::default(),
            None,
            Format::Csv,
            &mut out,
        )
        .expect("export");

        let text = String::from_utf8(out).expect("utf8");
        assert_eq!(n, 1);
        assert!(
            text.contains(r#""echo ""hi, there""""#),
            "quotes must be doubled and the field quoted: {text}"
        );
        assert_eq!(text.lines().count(), 2, "header plus one row");
    }

    #[test]
    fn json_export_is_a_single_parseable_array() {
        let db = seeded();
        let mut out = Vec::new();
        export(
            db.conn(),
            &TimelineFilter::default(),
            None,
            Format::Json,
            &mut out,
        )
        .expect("export");

        let parsed: serde_json::Value =
            serde_json::from_slice(&out).expect("the export must be valid JSON");
        assert_eq!(parsed.as_array().expect("array").len(), 1);
        assert_eq!(parsed[0]["tool_use_id"], "toolu_1");
    }

    #[test]
    fn an_empty_store_still_exports_valid_json() {
        let db = Db::open_in_memory().expect("db");
        let mut out = Vec::new();
        export(
            db.conn(),
            &TimelineFilter::default(),
            None,
            Format::Json,
            &mut out,
        )
        .expect("export");
        let parsed: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");
        assert!(parsed.as_array().expect("array").is_empty());
    }

    #[test]
    fn a_limit_stops_the_export_where_it_says() {
        let db = Db::open_in_memory().expect("db");
        for i in 0..5 {
            project::upsert_transcript(
                db.conn(),
                &format!("toolu_{i}"),
                &TranscriptFacts {
                    called_at: Some(i),
                    ..TranscriptFacts::default()
                },
            )
            .expect("project");
        }
        let mut out = Vec::new();
        let n = export(
            db.conn(),
            &TimelineFilter::default(),
            Some(2),
            Format::Jsonl,
            &mut out,
        )
        .expect("export");
        assert_eq!(n, 2);
        assert_eq!(String::from_utf8(out).expect("utf8").lines().count(), 2);
    }

    #[test]
    fn every_export_says_which_lanes_witnessed_each_row() {
        let db = seeded();
        // A refusal: OTEL saw it, and only OTEL knows who refused it.
        project::upsert_otel(
            db.conn(),
            "toolu_1",
            &toolog_core::model::OtelFacts {
                duration_ms: Some(41),
                decision: Some("reject".to_string()),
                decision_source: Some("user_reject".to_string()),
                ..toolog_core::model::OtelFacts::default()
            },
        )
        .expect("otel");

        let render = |format| {
            let mut out = Vec::new();
            export(
                db.conn(),
                &TimelineFilter::default(),
                None,
                format,
                &mut out,
            )
            .expect("export");
            String::from_utf8(out).expect("utf8")
        };

        let csv = render(Format::Csv);
        assert!(csv.lines().next().expect("header").contains("lanes"));
        assert!(
            csv.contains("\"both\""),
            "provenance travels with the row: {csv}"
        );

        let md = render(Format::Markdown);
        assert!(md.starts_with("# toolog export"));
        assert!(md.contains("| Time (UTC) |"));
        assert!(
            md.contains("| Bash | refused | 41 ms | user_reject | both |"),
            "a refusal must read as one, not as a success: {md}"
        );
        assert!(
            md.contains("`transcript` means no decision"),
            "an export that does not say how completely it was observed is worth less"
        );
    }

    /// Shell commands contain pipes, and a pipe ends a Markdown table cell.
    #[test]
    fn a_markdown_cell_survives_a_shell_pipeline() {
        let db = Db::open_in_memory().expect("db");
        project::upsert_transcript(
            db.conn(),
            "toolu_p",
            &TranscriptFacts {
                tool_name: Some("Bash".to_string()),
                input_summary: Some("ps aux | grep -v grep\nwc -l".to_string()),
                called_at: Some(1),
                ..TranscriptFacts::default()
            },
        )
        .expect("project");

        let mut out = Vec::new();
        export(
            db.conn(),
            &TimelineFilter::default(),
            None,
            Format::Markdown,
            &mut out,
        )
        .expect("export");
        let text = String::from_utf8(out).expect("utf8");
        let row = text
            .lines()
            .find(|l| l.contains("ps aux"))
            .expect("the row");
        assert!(row.contains(r"grep -v grep"));
        assert_eq!(
            row.matches(" | ").count(),
            6,
            "seven columns, whatever the command contains: {row}"
        );
        assert!(!row.contains('\n'));
    }

    /// The filter behind `toolog export --rejected`, which for two phases
    /// selected the OTLP lane rather than the refusals in it.
    #[test]
    fn the_rejected_filter_selects_refusals_not_a_lane() {
        let db = Db::open_in_memory().expect("db");
        for (id, decision) in [("toolu_ok", "accept"), ("toolu_no", "reject")] {
            project::upsert_transcript(
                db.conn(),
                id,
                &TranscriptFacts {
                    tool_name: Some("Bash".to_string()),
                    called_at: Some(1),
                    ..TranscriptFacts::default()
                },
            )
            .expect("transcript");
            project::upsert_otel(
                db.conn(),
                id,
                &toolog_core::model::OtelFacts {
                    decision: Some(decision.to_string()),
                    decision_source: Some("config".to_string()),
                    ..toolog_core::model::OtelFacts::default()
                },
            )
            .expect("otel");
        }

        // Both rows carry the OTLP bit; only one of them was refused.
        let mut out = Vec::new();
        let n = export(db.conn(), &rejected_only(), None, Format::Jsonl, &mut out).expect("export");
        assert_eq!(n, 1, "a refusal is a decision, not a provenance");
        assert!(String::from_utf8(out).expect("utf8").contains("toolu_no"));
    }

    /// Task 6.8 as an assertion on the words, not on a number: a store with no
    /// cost data must not be rendered as one that cost nothing.
    #[test]
    fn a_store_with_no_cost_says_so_rather_than_reporting_zero() {
        let db = seeded();
        let window = usage_window(None, None);
        let report = usage(&db, &window).expect("usage");
        let text = render_usage(&report);

        assert!(text.contains("No cost captured."), "{text}");
        assert!(!text.contains("$0.00"), "{text}");
        assert!(
            text.contains("imported from transcripts, which record no cost"),
            "{text}"
        );
    }

    #[test]
    fn an_empty_window_reports_nothing_rather_than_a_table_of_zeroes() {
        let db = Db::open_in_memory().expect("db");
        let report = usage(&db, &usage_window(Some(7), None)).expect("usage");
        assert_eq!(render_usage(&report), "No calls in this window.\n");
    }

    #[test]
    fn a_window_of_days_is_absolute_once_resolved() {
        let bounded = usage_window(Some(7), Some("/work/app".to_string()));
        let since = bounded.since.expect("since");
        let until = bounded.until.expect("until");
        assert_eq!(until - since, 7 * 86_400_000);
        assert_eq!(bounded.project_path.as_deref(), Some("/work/app"));

        let all = usage_window(None, None);
        assert!(
            all.since.is_none() && all.until.is_none(),
            "the whole store has no bounds, so it has nothing to compare with"
        );
    }

    /// Task 7.4's whole point: the preview names what would go, and nothing
    /// about running it removes anything.
    #[test]
    fn a_purge_preview_names_the_sessions_and_says_nothing_was_deleted() {
        use toolog_core::model::Session;
        use toolog_core::retention::{self, Scope};

        let db = Db::open_in_memory().expect("db");
        project::upsert_session(
            db.conn(),
            &Session {
                session_id: "s1".into(),
                project_path: Some("/work/app".into()),
                transcript_path: Some("/t/s1.jsonl".into()),
                last_seen: Some(1_000),
                ..Session::default()
            },
        )
        .expect("session");

        let plan =
            retention::preview(db.conn(), &Scope::Before { cutoff_ms: 2_000 }).expect("preview");
        let text = render_purge(&plan, false);

        assert!(text.contains("s1"), "{text}");
        assert!(text.contains("/work/app"), "{text}");
        assert!(text.contains("This is a preview"), "{text}");
        assert!(text.contains("--apply"), "{text}");
        assert!(!text.contains("would remove 0 of"), "{text}");
    }

    #[test]
    fn a_purge_that_would_remove_nothing_says_so_rather_than_printing_a_table() {
        use toolog_core::retention::{self, Scope};

        let db = Db::open_in_memory().expect("db");
        let plan = retention::preview(db.conn(), &Scope::Before { cutoff_ms: 1 }).expect("preview");
        let text = render_purge(&plan, false);

        assert!(text.starts_with("Nothing to remove"), "{text}");
        assert!(!text.contains("--apply"), "an offer to apply nothing");
    }

    #[test]
    fn sizes_are_reported_in_units_a_person_reads() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(2048), "2.0 KiB");
        assert_eq!(bytes(3 * 1024 * 1024), "3.0 MiB");
        assert_eq!(bytes(5 * 1024 * 1024 * 1024), "5.00 GiB");
    }
}
