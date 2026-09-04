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
use toolog_core::model::{
    FileChange, Page, Reconciliation, SearchHit, Session, TimelineFilter, ToolCall,
};
use toolog_core::query;
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
}

/// Everything the analytics view opens with.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "unused/")]
pub(crate) struct Stats {
    pub(crate) totals: query::Totals,
    pub(crate) tools: Vec<query::ToolUsage>,
    /// Lane divergence. Not a footnote: OTEL-only calls are refusals.
    pub(crate) reconciliation: Reconciliation,
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

commands! {
    |app, handle|

    /// One page of the timeline, newest first.
    query_timeline(filter: TimelineFilter, page: Page) -> Vec<ToolCall> {
        app.read(|c| query::timeline_page(c, &filter, page))
    }

    /// How many calls match a filter, ignoring paging.
    ///
    /// Beyond the tasks' list, and needed by every one of them: a virtualized
    /// list cannot size its scrollbar without it.
    timeline_count(filter: TimelineFilter) -> i64 {
        app.read(|c| query::timeline_count(c, &filter))
    }

    /// One call and the files it changed.
    get_tool_call(tool_use_id: String) -> Option<ToolCallDetail> {
        app.read(|c| {
            let Some(call) = query::tool_call_detail(c, &tool_use_id)? else {
                return Ok(None);
            };
            Ok(Some(ToolCallDetail {
                file_changes: query::file_changes(c, &tool_use_id)?,
                call,
            }))
        })
    }

    /// Sessions, most recently active first.
    list_sessions(page: Page) -> Vec<Session> {
        app.read(|c| query::list_sessions(c, page))
    }

    /// Totals, per-tool usage and lane reconciliation.
    stats() -> Stats {
        app.read(|c| {
            Ok(Stats {
                totals: query::stats_totals(c)?,
                tools: query::stats_tool_usage(c)?,
                reconciliation: query::reconcile(c)?,
            })
        })
    }

    /// Full-text search over commands, paths and result text.
    search(input: String, page: Page) -> Vec<SearchHit> {
        app.read(|c| query::search(c, &input, page))
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
        app.read(|c| {
            let mut out = Vec::new();
            cli::export(c, &filter, limit, format, &mut out)?;
            Ok(String::from_utf8_lossy(&out).into_owned())
        })
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

    /// Show the log directory in the file manager.
    reveal_logs() -> () {
        let dir = toolog_cli::logging::log_dir()?;
        std::fs::create_dir_all(&dir)?;
        window::reveal(&handle, &dir)
    }
}
