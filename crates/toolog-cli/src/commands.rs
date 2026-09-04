//! What the CLI subcommands actually do.
//!
//! Kept apart from argument parsing so the Tauri commands can call the same
//! functions: "Run Backfill" in the tray and `toolog backfill` on the command
//! line must not be two implementations that drift.

use std::io::Write;
use std::path::{Path, PathBuf};

use toolog_core::model::{Page, TimelineFilter, provenance};
use toolog_core::{Connection, Db, Result, query};
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
            "called_at,session_id,tool_name,success,duration_ms,decision,decision_source,target_path,input_summary"
        )?;
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
                        "{},{},{},{},{},{},{},{},{}",
                        row.called_at.map(|t| t.to_string()).unwrap_or_default(),
                        csv(row.session_id.as_deref()),
                        csv(row.tool_name.as_deref()),
                        row.success.map(|s| s.to_string()).unwrap_or_default(),
                        row.duration_ms.map(|d| d.to_string()).unwrap_or_default(),
                        csv(row.decision.as_deref()),
                        csv(row.decision_source.as_deref()),
                        csv(row.target_path.as_deref()),
                        csv(row.input_summary.as_deref()),
                    )?;
                }
            }
            written += 1;
        }

        offset += u32::try_from(rows.len()).unwrap_or(PAGE);
    }

    if format == Format::Json {
        writeln!(out, "\n]")?;
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

/// Only calls that OTEL saw and the transcript did not — the refusals.
#[must_use]
pub fn rejected_only() -> TimelineFilter {
    TimelineFilter {
        provenance_mask: Some(provenance::OTLP),
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
    fn the_rejected_filter_selects_the_otel_only_lane() {
        assert_eq!(rejected_only().provenance_mask, Some(provenance::OTLP));
    }
}
