//! The Tauri command surface, and the TypeScript that calls it.
//!
//! Both come from one declaration. A command's name, its arguments and its
//! return type are written once in [`commands!`], which generates the Rust
//! handler *and* the typed wrapper the frontend imports — so the boundary
//! cannot drift, which is the whole point of task 4.9. Adding a command by
//! hand somewhere else would compile, but it would not appear in the bindings,
//! and the check in `bindings.rs` fails the build when the checked-in
//! TypeScript no longer matches.
//!
//! Every command runs on a blocking thread. SQLite calls block, and blocking
//! the WebView's event loop for even a slow query is how a desktop application
//! earns a reputation for jank.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use toolog_cli::capture;
use toolog_cli::commands::{self as cli, Format};
use toolog_cli::prefs::Prefs;
use toolog_core::model::{FileChange, Page, Session, TimelineFilter, ToolCall};
use toolog_core::rules::{Finding, ProjectRisk, SeverityTally};
use toolog_core::{query, raw, rules};
use ts_rs::TS;

use crate::state::AppState;
use crate::window;

/// A tool call with everything the detail pane needs, in one round trip.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "unused/")]
pub(crate) struct ToolCallDetail {
    pub(crate) call: ToolCall,
    /// Files this call changed, with their diffs.
    pub(crate) file_changes: Vec<FileChange>,
    /// The envelope the call ran inside: cwd, branch, Claude Code version.
    /// `None` for a call whose session the store never learned.
    pub(crate) session: Option<Session>,
}

/// Where a call's evidence sits on disk, for "open the transcript".
///
/// Deliberately not part of [`ToolCallDetail`]: finding it is a scan over one
/// transcript's stored lines, and the detail pane follows the selection as the
/// user arrows down the list. It is fetched when the source is asked for.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "unused/")]
pub(crate) struct SourceView {
    /// The transcript that recorded this call.
    pub(crate) path: String,
    /// 1-based line number, when the file is still on disk to count into.
    pub(crate) line: Option<u32>,
    /// Whether that file is still there. Transcripts are Claude Code's to
    /// delete, and the stored record outlives them.
    pub(crate) exists: bool,
    /// The stored transcript line, verbatim. This is the evidence; the file is
    /// a convenience.
    pub(crate) body: String,
}

/// Count the lines before a byte offset, for a 1-based line number.
///
/// Reads only as far as the offset rather than the whole transcript, which in
/// this corpus runs to tens of megabytes.
fn line_at(path: &std::path::Path, offset: i64) -> std::io::Result<u32> {
    use std::io::{BufRead as _, Read as _};

    let take = u64::try_from(offset).unwrap_or(0);
    let mut reader = std::io::BufReader::new(std::fs::File::open(path)?).take(take);
    let mut line = Vec::new();
    let mut lines = 1u32;
    while reader.read_until(b'\n', &mut line)? > 0 {
        if line.last() != Some(&b'\n') {
            // The offset landed inside a line, which is still that line.
            break;
        }
        lines = lines.saturating_add(1);
        line.clear();
    }
    Ok(lines)
}

/// Everything the risk view opens with (tasks 6.3, 6.4, 11.7–11.12).
///
/// The findings, the summary and the per-project table are computed in one
/// pass over one rule set. Two commands would let the summary describe a
/// different set of rules from the list below it, which is exactly the
/// disagreement a review cannot have — and in v1.0 it did have one, because
/// the summary counted rules and the table counted (rule, project) pairs.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "unused/")]
pub(crate) struct RiskReview {
    /// Every rule, worst first — including the ones that matched nothing
    /// (task 11.11). Dismissed findings keep their place and are marked.
    pub(crate) findings: Vec<Finding>,
    /// The four numbers the page opens with, in distinct calls flagged.
    pub(crate) totals: Vec<SeverityTally>,
    /// One row per project, whose severity columns add up to those numbers.
    pub(crate) projects: Vec<ProjectRisk>,
    /// Where a user rules file would go, whether or not one is there. The view
    /// says so, because "rules are data you can edit" is only true if you can
    /// find the file.
    pub(crate) rules_path: Option<String>,
    /// Whether that file exists and was loaded.
    pub(crate) rules_customized: bool,
    /// Whether this is the first review this store has ever had (task 12.5).
    ///
    /// "Nobody has looked yet" and "nothing was found" are different
    /// statements, and reporting the first as "0 new findings" reads as
    /// reassurance it has not earned.
    pub(crate) first_review: bool,
}

/// When the user's rules file was last written, or `None` if there is none.
///
/// One of the three things the memo is guarded by. "No file" is itself a state
/// that can change — writing a rules file for the first time must retire the
/// memo — so this is `Option<Option<_>>` collapsed: `None` means no file, and
/// that is a value that compares unequal to any mtime.
fn rules_mtime() -> Option<std::time::SystemTime> {
    cli::rules_path()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
}

/// The review, computed or remembered (task 11.3).
///
/// Re-opening the tab with nothing newly captured issues one `PRAGMA
/// data_version` and reads an atomic, rather than running twelve rules over
/// the whole store. The pragma is read on the risk connection, which is the
/// reason that connection exists: it reports commits by *other* connections,
/// and would never move on the one doing the writing.
fn risk_review(app: &AppState) -> anyhow::Result<RiskReview> {
    let mtime = rules_mtime();
    if let Some(cached) = app.cached_risk(mtime) {
        return Ok(cached);
    }

    let path = cli::rules_path();
    let customized = path.as_ref().is_some_and(|p| p.is_file());
    let rules = cli::rules()?;
    let first_review = app.read_risk(|c| Ok(!rules::ever_reviewed(c)?))?;

    let (mut findings, reconciled) = app.read_risk(|c| {
        let findings = rules::evaluate(c, &rules)?;
        let reconciled = rules::reconcile(c, &rules, &findings)?;
        Ok((findings, reconciled))
    })?;

    // Task 12.3: a review records what it saw, which makes it a *mutating*
    // command. The write goes through the single writer (ADR-0007), the way a
    // dismissal does — the risk connection is a reader.
    let now = raw::now_ms();
    let recorded = {
        let mut seen = std::mem::take(&mut findings);
        let for_writer = rules.clone();
        let out = app.with_capture(move |capture| {
            capture
                .writer()
                .submit_blocking(move |conn| {
                    rules::record_sightings(conn, &for_writer, &mut seen, now).map(|_| seen)
                })
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .map_err(anyhow::Error::from)
        });
        match out {
            Ok(seen) => seen,
            // A store that cannot be written to is still a store worth
            // reviewing. The findings are right; only the ledger is behind.
            Err(error) => {
                tracing::warn!(%error, "sightings not recorded");
                app.read_risk(|c| {
                    let mut findings = rules::evaluate(c, &rules)?;
                    rules::read_sightings(c, &rules, &mut findings)?;
                    Ok(findings)
                })?
            }
        }
    };

    let review = RiskReview {
        findings: recorded,
        totals: reconciled.totals,
        projects: reconciled.projects,
        rules_path: path.map(|p| p.display().to_string()),
        rules_customized: customized,
        first_review,
    };

    // Taken **after** the sighting write, which inverts task 11.3's rule and
    // for the opposite reason: that write is ours and is already accounted for
    // in the answer above, so stamping the watermark before it would leave a
    // memo that expires immediately on our own change.
    let watermark = app.risk_watermark()?;
    app.remember_risk(watermark, mtime, &review);
    Ok(review)
}

/// What the first-run wizard and the Preferences pane show.
///
/// A flattened view of `doctor`'s report: the booleans drive the UI, and
/// `report` is the same text `toolog doctor` prints, so the two can never tell
/// different stories.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "unused/")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a status report, not a state machine: each flag is an independent \
              observation the wizard renders as its own line"
)]
pub(crate) struct Setup {
    pub(crate) configured: bool,
    pub(crate) listening: bool,
    pub(crate) endpoint: String,
    pub(crate) settings_path: String,
    pub(crate) transcripts_dir: String,
    pub(crate) transcript_files: u32,
    pub(crate) ingested_files: i64,
    pub(crate) agent_supported: bool,
    pub(crate) agent_installed: bool,
    pub(crate) problems: Vec<String>,
    /// The rendered `toolog doctor` output, verbatim.
    pub(crate) report: String,
}

/// What removing toolog would do, for the window (task 8.6).
///
/// `report` is the same text `toolog uninstall` prints. The window shows it
/// verbatim rather than rebuilding the explanation in TypeScript, because the
/// two must not be able to describe the same irreversible action differently.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "unused/")]
pub(crate) struct UninstallPlan {
    pub(crate) report: String,
    /// Whether applying it would change anything at all.
    pub(crate) any_changes: bool,
    /// Recorded history, in bytes, so the button can name what it would delete.
    pub(crate) data_bytes: i64,
    /// Where that history lives.
    pub(crate) data_dir: String,
    /// The settings file goes back byte for byte, rather than being edited.
    pub(crate) restores_backup: bool,
}

/// What an applied uninstall did, step by step.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "unused/")]
pub(crate) struct UninstallOutcome {
    pub(crate) done: Vec<String>,
    pub(crate) failed: Vec<String>,
}

fn uninstall_plan(delete_data: bool) -> toolog_cli::uninstall::Plan {
    let home = toolog_cli::settings::home_dir();
    let cwd = std::env::current_dir().unwrap_or_else(|_| home.clone());
    toolog_cli::uninstall::plan(&home, &cwd, delete_data)
}

fn setup_now() -> anyhow::Result<Setup> {
    let paths = toolog_cli::doctor::Paths::detect()?;
    let report = toolog_cli::doctor::report(&paths);
    Ok(Setup {
        configured: report.configured(),
        listening: report.health.is_up(),
        endpoint: report.endpoint.clone(),
        settings_path: report.settings_path.display().to_string(),
        transcripts_dir: report.transcripts.dir.display().to_string(),
        transcript_files: u32::try_from(report.transcripts.files).unwrap_or(u32::MAX),
        ingested_files: report.transcripts.ingested_files,
        agent_supported: report.agent.supported,
        agent_installed: report.agent.installed,
        problems: report.problems(),
        report: toolog_cli::doctor::render(&report),
    })
}

/// Declare the IPC surface once, in Rust and TypeScript together.
///
/// Each entry expands to an `async` Tauri command that runs `body` on a
/// blocking thread with `app: &AppState` and `handle: tauri::AppHandle` in
/// scope, and contributes one typed wrapper to the generated bindings.
macro_rules! commands {
    (
        // The two names every body may use, passed in from the call site so
        // macro hygiene does not hide them.
        |$app:ident, $handle:ident|
        $(
            $(#[$meta:meta])*
            $name:ident ( $( $arg:ident : $argty:ty ),* $(,)? ) -> $ret:ty $body:block
        )*
    ) => {
        $(
            $(#[$meta])*
            #[tauri::command]
            pub(crate) async fn $name(
                $handle: tauri::AppHandle,
                state: tauri::State<'_, Arc<AppState>>,
                $( $arg: $argty, )*
            ) -> Result<$ret, String> {
                let state = Arc::clone(&state);
                tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<$ret> {
                    let _ = &$handle;
                    #[allow(unused_variables)]
                    let $app: &AppState = &state;
                    $body
                })
                .await
                // A panic in a command must reach the UI as an error, not a
                // silently pending promise.
                .map_err(|e| format!("command failed: {e}"))?
                .map_err(|e| format!("{e:#}"))
            }
        )*

        /// The handler Tauri registers.
        pub(crate) fn handler() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
            tauri::generate_handler![$($name),*]
        }

        /// One typed wrapper per command, for the generated bindings.
        #[cfg(test)]
        pub(crate) fn signatures(cfg: &ts_rs::Config) -> Vec<crate::bindings::Signature> {
            vec![$(
                crate::bindings::Signature {
                    rust_name: stringify!($name),
                    args: vec![$(
                        (stringify!($arg), <$argty as TS>::name(cfg)),
                    )*],
                    ret: <$ret as TS>::name(cfg),
                },
            )*]
        }
    };
}

/// A timeline filter, with the rules its `@risk` and `@rule` fields need.
///
/// Every read the timeline makes goes through this rather than through
/// `Lens::plain`, so a filter naming a risk field can never reach the query
/// layer without the rules that give it meaning (task 12.7). Reading the rules
/// file is a few microseconds; getting this wrong is a wrong answer.
fn timeline_lens<T>(
    app: &AppState,
    filter: &TimelineFilter,
    f: impl FnOnce(&toolog_core::Connection, query::Lens<'_>) -> toolog_core::Result<T>,
) -> anyhow::Result<T> {
    // A filter that asks for nothing rule-shaped does not pay for the file.
    if filter.risk.is_none() && filter.rule_id.is_none() {
        return app.read(|c| f(c, query::Lens::plain(filter)));
    }
    let rules = cli::rules()?;
    app.read(|c| {
        let dismissed = rules::dismissed_rules(c)?;
        f(c, query::Lens::with_rules(filter, &rules, &dismissed))
    })
}

commands! {
    |app, handle|

    /// One page of the timeline, newest first.
    ///
    /// Rows carry the session facts they display and, when the filter carried
    /// a search term, the snippet showing where the match was — the list never
    /// makes a second call to draw a row.
    query_timeline(filter: TimelineFilter, page: Page) -> Vec<query::TimelineRow> {
        timeline_lens(app, &filter, |c, lens| query::timeline_rows(c, lens, page))
    }

    /// The distinct values the filter controls offer.
    facets() -> query::Facets {
        app.read(query::facets)
    }

    /// Every rule id in force, for the query bar's `@rule:` completions.
    ///
    /// Its own command rather than a field on `facets`: facets come from the
    /// store and rules come from a file, and a call that mixed the two would
    /// have to explain which half was stale.
    rule_ids() -> Vec<String> {
        Ok(cli::rules()?.into_iter().map(|r| r.id).collect())
    }

    /// The activity histogram over the same filter the list is showing.
    ///
    /// `utcOffsetMinutes` is the window's own — `-new Date().getTimezoneOffset()`
    /// — because a day boundary is a local fact and the store keeps UTC. The
    /// bucket size is chosen from the span rather than passed in: it is a
    /// property of what is being looked at, not a preference.
    timeline_histogram(filter: TimelineFilter, utc_offset_minutes: i32) -> query::Histogram {
        timeline_lens(app, &filter, |c, lens| query::histogram(c, lens, utc_offset_minutes))
    }

    /// How many calls match a filter, ignoring paging.
    ///
    /// Beyond the tasks' list, and needed by every one of them: a virtualized
    /// list cannot size its scrollbar without it.
    timeline_count(filter: TimelineFilter) -> i64 {
        timeline_lens(app, &filter, |c, lens| query::timeline_count(c, lens))
    }

    /// One call, the files it changed, and the session it ran in.
    get_tool_call(tool_use_id: String) -> Option<ToolCallDetail> {
        app.read(|c| {
            let Some(call) = query::tool_call_detail(c, &tool_use_id)? else {
                return Ok(None);
            };
            let session = match call.session_id.as_deref() {
                Some(id) => query::session(c, id)?,
                None => None,
            };
            Ok(Some(ToolCallDetail {
                file_changes: query::file_changes(c, &tool_use_id)?,
                session,
                call,
            }))
        })
    }

    /// The transcript line that recorded a call, and where to find it.
    get_source(tool_use_id: String) -> Option<SourceView> {
        app.read(|c| {
            let Some(call) = query::tool_call_detail(c, &tool_use_id)? else {
                return Ok(None);
            };
            let Some(record) = query::source_record(c, &call)? else {
                return Ok(None);
            };
            let path = std::path::PathBuf::from(&record.source_ref);
            let exists = path.is_file();
            let line = record
                .source_offset
                .filter(|_| exists)
                .and_then(|offset| line_at(&path, offset).ok());
            Ok(Some(SourceView {
                path: record.source_ref,
                line,
                exists,
                body: record.body,
            }))
        })
    }

    /// Sessions, most recently active first.
    list_sessions(page: Page) -> Vec<Session> {
        app.read(|c| query::list_sessions(c, page))
    }

    /// Whether capture is running, and how much it has taken in.
    collector_status() -> capture::Status {
        app.with_capture(|capture| Ok(capture.status()?))
    }

    /// Stop or resume storing records.
    set_paused(paused: bool) -> capture::Status {
        app.with_capture(|capture| {
            if paused { capture.pause(); } else { capture.resume(); }
            Ok(capture.status()?)
        })
    }

    /// Import existing history.
    ///
    /// Runs on the shared writer so it cannot race the live lanes.
    run_backfill() -> cli::Summary {
        app.with_capture(|capture| {
            capture
                .writer()
                .submit_blocking(|conn| {
                    let mut projector = toolog_ingest::TranscriptProjector::new();
                    let root = toolog_ingest::discover::projects_dir();
                    let mut summary = cli::Summary::default();
                    let Some(root) = root else { return Ok(summary) };
                    for path in toolog_ingest::discover::transcripts(&root) {
                        let report = toolog_ingest::backfill::ingest_and_project(
                            conn, &path, &mut projector,
                        )?;
                        summary.files += 1;
                        summary.lines += report.lines;
                        summary.stored += report.stored;
                        summary.duplicates += report.lines - report.stored;
                    }
                    let stats = projector.stats();
                    summary.tool_uses = stats.tool_uses;
                    summary.sessions = stats.sessions;
                    Ok::<_, toolog_core::Error>(summary)
                })
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .map_err(Into::into)
        })
    }

    /// Serialize matching calls for saving or pasting into a report.
    ///
    /// Not `export`: that is a reserved word in TypeScript, and the generated
    /// wrapper would not parse. The bindings test enforces the rule rather than
    /// leaving it to memory.
    export_calls(filter: TimelineFilter, format: Format, limit: Option<u32>) -> String {
        timeline_lens(app, &filter, |c, lens| {
            let mut out = Vec::new();
            cli::export(c, lens, limit, format, &mut out)?;
            Ok(String::from_utf8_lossy(&out).into_owned())
        })
    }

    /// Write the current filter to a file the user chooses.
    ///
    /// The only path this process writes outside its own store, and it comes
    /// from a native save panel rather than from the WebView. Returns `None`
    /// when the panel was dismissed.
    save_export(filter: TimelineFilter, format: Format, limit: Option<u32>) -> Option<String> {
        use tauri_plugin_dialog::DialogExt;

        let (ext, label) = match format {
            Format::Json => ("json", "JSON"),
            Format::Jsonl => ("jsonl", "JSON Lines"),
            Format::Csv => ("csv", "CSV"),
            Format::Markdown => ("md", "Markdown"),
        };
        let Some(chosen) = handle
            .dialog()
            .file()
            .set_file_name(format!("{}.{ext}", cli::export_file_stem()))
            .add_filter(label, &[ext])
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = chosen
            .into_path()
            .map_err(|e| anyhow::anyhow!("that location cannot be written to: {e}"))?;

        let mut file = std::fs::File::create(&path)?;
        timeline_lens(app, &filter, |c, lens| {
            cli::export(c, lens, limit, format, &mut file)?;
            Ok(())
        })?;
        Ok(Some(path.display().to_string()))
    }

    /// The risk review: what the rules found, and each project's posture.
    risk() -> RiskReview {
        risk_review(app)
    }

    /// Every call one rule matched, newest first.
    ///
    /// The drill-through of task 6.3. A finding carries eight examples for
    /// reading; this is for leaving the finding and going through the rest.
    rule_calls(rule_id: String, page: Page) -> Vec<ToolCall> {
        let rule = cli::rules()?
            .into_iter()
            .find(|r| r.id == rule_id)
            .ok_or_else(|| anyhow::anyhow!("no rule with id {rule_id}"))?;
        app.read(|c| rules::calls(c, &rule, page))
    }

    /// Set a rule aside, with the reason someone had for it.
    ///
    /// A dismissal hides nothing: the finding keeps its place in the list and
    /// carries the note. What it changes is the per-project posture, which is
    /// a judgement about what still needs answering.
    dismiss_rule(rule_id: String, note: String) -> RiskReview {
        app.with_capture(|capture| {
            capture
                .writer()
                .submit_blocking(move |conn| rules::dismiss(conn, &rule_id, &note, raw::now_ms()))
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .map_err(anyhow::Error::from)
        })?;
        app.note_dismissal();
        risk_review(app)
    }

    /// Undo a dismissal. The calls behind it were never touched either way.
    restore_rule(rule_id: String) -> RiskReview {
        app.with_capture(|capture| {
            capture
                .writer()
                .submit_blocking(move |conn| rules::restore(conn, &rule_id))
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .map_err(anyhow::Error::from)
        })?;
        app.note_dismissal();
        risk_review(app)
    }

    /// Which notifications are switched on. Both off until someone says so.
    get_prefs() -> Prefs {
        Ok(app.prefs())
    }

    /// Turn a notification on or off, and remember it across restarts.
    set_prefs(prefs: Prefs) -> Prefs {
        app.set_prefs(prefs)
    }

    /// The state of the Claude Code integration, for the wizard.
    doctor_status() -> Setup {
        setup_now()
    }

    /// Write the telemetry configuration, then re-check.
    ///
    /// The one command that writes a file the application does not own, and it
    /// is only ever reached from an explicit click.
    apply_doctor_fix() -> Setup {
        let paths = toolog_cli::doctor::Paths::detect()?;
        toolog_cli::doctor::fix(&paths)?;
        setup_now()
    }

    /// Install or remove the login agent. Never silent, never on by default.
    set_login_agent(install: bool) -> Setup {
        let home = toolog_cli::settings::home_dir();
        if install {
            let exe = std::env::current_exe()?;
            let log_dir = toolog_cli::logging::log_dir()?;
            std::fs::create_dir_all(&log_dir)?;
            toolog_cli::launchagent::install(&home, &exe, &log_dir)?;
        } else {
            toolog_cli::launchagent::uninstall(&home)?;
        }
        setup_now()
    }

    /// What removing toolog would do. Reads only; changes nothing.
    uninstall_preview(delete_data: bool) -> UninstallPlan {
        let plan = uninstall_plan(delete_data);
        Ok(UninstallPlan {
            report: toolog_cli::commands::render_uninstall(&plan, false),
            any_changes: !plan.is_empty(),
            data_bytes: i64::try_from(plan.data_bytes()).unwrap_or(i64::MAX),
            data_dir: plan.data_dir.as_deref().unwrap_or(std::path::Path::new("")).display().to_string(),
            restores_backup: matches!(
                plan.settings,
                toolog_cli::uninstall::SettingsRevert::RestoreBackup { .. }
            ),
            })
    }

    /// Carry out the uninstall the preview described.
    ///
    /// The plan is recomputed rather than passed in from the window: what the
    /// WebView holds is a description of a moment that has passed, and the
    /// only safe thing to act on is the state of the disk right now.
    uninstall_run(delete_data: bool) -> UninstallOutcome {
        let home = toolog_cli::settings::home_dir();
        let plan = uninstall_plan(delete_data);
        let outcome = toolog_cli::uninstall::apply(&home, &plan);
        Ok(UninstallOutcome {
            done: outcome.done,
            failed: outcome.failed,
        })
    }

    /// Show the log directory in the file manager.
    reveal_logs() -> () {
        let dir = toolog_cli::logging::log_dir()?;
        std::fs::create_dir_all(&dir)?;
        window::reveal(&handle, &dir)
    }

    /// Show the rules file in the file manager (task 11.13).
    ///
    /// "Rules are data you can edit" is only true if you can find them, and a
    /// footnote naming a path is not finding them. Where no file exists yet one
    /// is created first — commented out entirely, so the rules in force before
    /// and after are identical — because revealing an empty folder answers a
    /// question nobody asked.
    reveal_rules() -> () {
        let path = cli::ensure_rules_file()?;
        window::reveal(&handle, &path)
    }

    /// Show a transcript in the file manager.
    ///
    /// Confined to `~/.claude/projects`. The frontend only ever passes a path
    /// this process put there, but "reveal whatever the WebView asks for" is
    /// not a command worth having, and the check costs one comparison.
    reveal_transcript(path: String) -> () {
        let path = std::path::PathBuf::from(path);
        let root = toolog_ingest::discover::projects_dir()
            .ok_or_else(|| anyhow::anyhow!("no transcripts directory on this machine"))?;
        if !path.starts_with(&root) {
            anyhow::bail!("{} is not a transcript", path.display());
        }
        window::reveal(&handle, &path)
    }
}
