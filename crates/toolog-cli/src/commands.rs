//! What the CLI subcommands actually do.
//!
//! Kept apart from argument parsing so the Tauri commands can call the same
//! functions: "Run Backfill" in the tray and `toolog backfill` on the command
//! line must not be two implementations that drift.

use std::io::Write;
use std::path::{Path, PathBuf};

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
pub fn verify(db: &Db) -> Result<toolog_core::model::Reconciliation> {
    query::reconcile(db.conn())
}

/// Render a reconciliation the way an auditor reads it.
#[must_use]
pub fn render_verify(r: &toolog_core::model::Reconciliation) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    let _ = writeln!(out, "Lane reconciliation");
    let _ = writeln!(
        out,
        "  both lanes      {:>8}   content and decision, the complete record",
        r.both
    );
    let _ = writeln!(
        out,
        "  transcript only {:>8}   no decision, duration or cost — a collection gap",
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

    let total = r.total();
    if total > 0 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a percentage for display, not a computation anything depends on"
        )]
        let pct = r.both as f64 * 100.0 / total as f64;
        let _ = writeln!(out, "\n  {pct:.1}% of calls were witnessed by both lanes.");
    }
    let _ = writeln!(
        out,
        "\nTranscript-only calls are expected for history imported from before toolog ran.\n\
         Refusals are counted from the decision the OTLP lane carries, not from a missing\n\
         transcript: a denied call does leave a transcript record, but nothing in it says\n\
         who denied it or why."
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
}
