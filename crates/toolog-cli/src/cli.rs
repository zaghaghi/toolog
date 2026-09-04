//! Argument parsing and dispatch for the `toolog` binary.
//!
//! One artifact serves both the resident application and the command line
//! ([ADR-0007]), so this module defines the whole non-GUI surface and
//! `toolog-app` dispatches into it before touching Tauri. Running a command
//! must never start a window, and must never leave a resident process behind.
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

use std::io::Write;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::commands::{self, Format};
use crate::{doctor, launchagent, logging, settings};

/// A local audit trail for Claude Code tool calls.
#[derive(Debug, Parser)]
#[command(name = "toolog", version, about, long_about = None)]
pub struct Cli {
    /// Start resident with no window. Used by the login agent.
    #[arg(long, global = true)]
    pub background: bool,

    /// Database to use instead of the default location.
    #[arg(long, global = true, value_name = "PATH")]
    pub db: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Everything the binary can do without opening a window.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Report the state of the Claude Code integration.
    Doctor {
        /// Write the telemetry configuration into ~/.claude/settings.json.
        #[arg(long)]
        fix: bool,
    },
    /// Import existing history from ~/.claude/projects.
    Backfill {
        /// Directory to import instead of ~/.claude/projects.
        #[arg(long, value_name = "DIR")]
        path: Option<PathBuf>,
        /// Report only the totals.
        #[arg(long, short)]
        quiet: bool,
    },
    /// Cross-check the two ingestion lanes.
    Verify,
    /// Write tool calls to stdout or a file.
    Export(ExportArgs),
    /// Install or remove the login agent that keeps capture running.
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
}

/// Filters and output options for `toolog export`.
#[derive(Debug, Args)]
pub struct ExportArgs {
    #[arg(long, value_enum, default_value_t = Format::Json)]
    pub format: Format,
    /// Only this session.
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,
    /// Only this tool.
    #[arg(long, value_name = "NAME")]
    pub tool: Option<String>,
    /// Only calls newer than this: `7d`, `24h`, or an RFC 3339 timestamp.
    #[arg(long, value_name = "WHEN")]
    pub since: Option<String>,
    /// Only calls that were refused — the OTEL-only lane.
    #[arg(long)]
    pub rejected: bool,
    /// Stop after this many.
    #[arg(long, value_name = "N")]
    pub limit: Option<u32>,
    /// Write here instead of stdout.
    #[arg(long, short, value_name = "PATH")]
    pub output: Option<PathBuf>,
}

/// What to do with the login agent.
#[derive(Debug, Subcommand)]
pub enum AgentAction {
    /// Write the plist and load it.
    Install,
    /// Unload the job and delete the plist.
    Uninstall,
    /// Report whether it is installed and loaded.
    Status,
}

/// Run a command. Returns the process exit code.
///
/// A non-zero code for `doctor` is deliberate: it makes the command usable in a
/// script or a CI check, not only by eye.
pub fn run(cli: &Cli) -> anyhow::Result<i32> {
    logging::init_cli();

    match cli.command.as_ref().expect("a subcommand was given") {
        Command::Doctor { fix } => run_doctor(*fix),
        Command::Backfill { path, quiet } => run_backfill(cli, path.as_deref(), *quiet),
        Command::Verify => run_verify(cli),
        Command::Export(args) => run_export(cli, args),
        Command::Agent { action } => run_agent(action),
    }
}

fn db_path(cli: &Cli) -> anyhow::Result<PathBuf> {
    match &cli.db {
        Some(path) => Ok(path.clone()),
        None => Ok(commands::default_db_path()?),
    }
}

fn run_doctor(fix: bool) -> anyhow::Result<i32> {
    let paths = doctor::Paths::detect()?;

    if fix {
        match doctor::fix(&paths) {
            Ok(applied) => {
                if applied.plan.is_noop() {
                    println!("Already configured; {} unchanged.", applied.path.display());
                } else {
                    println!("Wrote {}", applied.path.display());
                    for (key, value) in &applied.plan.added {
                        println!("  + {key} = {value}");
                    }
                    for (key, from, to) in &applied.plan.changed {
                        println!("  ~ {key} = {to}   (was {from})");
                    }
                    if let Some(backup) = &applied.backup {
                        println!("  backup: {}", backup.display());
                    }
                    println!(
                        "\nRestart any running Claude Code session for the change to take effect."
                    );
                }
            }
            Err(e) => {
                eprintln!("{e}");
                return Ok(2);
            }
        }
    }

    let report = doctor::report(&paths);
    print!("{}", doctor::render(&report));
    Ok(i32::from(!report.problems().is_empty()))
}

fn run_backfill(cli: &Cli, path: Option<&std::path::Path>, quiet: bool) -> anyhow::Result<i32> {
    // ADR-0007 gives the process one writer. A backfill from the command line
    // while the application is resident makes two — safe under WAL, but they
    // contend, so say so rather than let it look like a slow import.
    if toolog_otlp::health::probe(toolog_otlp::port::default_addr()).is_up() {
        eprintln!(
            "note: toolog is already running. Import History in the tray uses its writer;\n\
             this command opens a second one and the two will contend for the lock.\n"
        );
    }

    let db = toolog_core::Db::open(db_path(cli)?)?;
    let started = std::time::Instant::now();

    let summary = commands::backfill(&db, path, |line| {
        if !quiet {
            println!("{line}");
        }
    })?;

    println!(
        "\n{} files, {} lines, {} new, {} already held.",
        summary.files, summary.lines, summary.stored, summary.duplicates
    );
    println!(
        "{} tool calls across {} sessions in {:.1}s.",
        summary.tool_uses,
        summary.sessions,
        started.elapsed().as_secs_f64()
    );
    Ok(0)
}

fn run_verify(cli: &Cli) -> anyhow::Result<i32> {
    let db = toolog_core::Db::open(db_path(cli)?)?;
    let reconciliation = commands::verify(&db)?;
    print!("{}", commands::render_verify(&reconciliation));
    Ok(0)
}

fn run_export(cli: &Cli, args: &ExportArgs) -> anyhow::Result<i32> {
    let db = toolog_core::Db::open(db_path(cli)?)?;

    let mut filter = if args.rejected {
        commands::rejected_only()
    } else {
        toolog_core::model::TimelineFilter::default()
    };
    filter.session_id.clone_from(&args.session);
    filter.tool_name.clone_from(&args.tool);
    if let Some(since) = &args.since {
        filter.since = Some(parse_since(since)?);
    }

    let mut sink: Box<dyn Write> = match &args.output {
        Some(path) => Box::new(std::io::BufWriter::new(std::fs::File::create(path)?)),
        None => Box::new(std::io::BufWriter::new(std::io::stdout().lock())),
    };
    let n = commands::export(db.conn(), &filter, args.limit, args.format, &mut sink)?;
    drop(sink);

    if let Some(path) = &args.output {
        eprintln!("{n} tool calls written to {}", path.display());
    }
    Ok(0)
}

fn run_agent(action: &AgentAction) -> anyhow::Result<i32> {
    let home = settings::home_dir();

    match action {
        AgentAction::Install => {
            let exe = std::env::current_exe()?;
            let log_dir = logging::log_dir()?;
            std::fs::create_dir_all(&log_dir)?;
            let path = launchagent::install(&home, &exe, &log_dir)?;
            println!("Installed {}", path.display());
            println!("toolog will start at login and restart if it crashes.");
            println!("Quitting from the tray still stops capture until the next login.");
        }
        AgentAction::Uninstall => {
            launchagent::uninstall(&home)?;
            println!("Removed {}", launchagent::plist_path(&home).display());
        }
        AgentAction::Status => {
            let status = launchagent::status(&home);
            if !status.supported {
                println!("Login agents are a macOS feature; nothing to report here.");
                return Ok(0);
            }
            println!("plist:     {}", status.path.display());
            println!("installed: {}", status.installed);
            println!(
                "loaded:    {}",
                status
                    .loaded
                    .map_or_else(|| "unknown".to_string(), |b| b.to_string())
            );
        }
    }
    Ok(0)
}

/// `7d`, `24h`, `30m`, or an RFC 3339 timestamp, to milliseconds since the epoch.
fn parse_since(input: &str) -> anyhow::Result<i64> {
    let trimmed = input.trim();

    if let Some((count, unit)) = trimmed.split_at_checked(trimmed.len().saturating_sub(1))
        && let Ok(n) = count.parse::<i64>()
    {
        let ms = match unit {
            "d" => Some(n * 86_400_000),
            "h" => Some(n * 3_600_000),
            "m" => Some(n * 60_000),
            _ => None,
        };
        if let Some(ms) = ms {
            return Ok(jiff::Timestamp::now().as_millisecond() - ms);
        }
    }

    let ts: jiff::Timestamp = trimmed.parse().map_err(|e| {
        anyhow::anyhow!("could not read {input:?} as a time: {e}. Try `7d`, `24h`, or a timestamp.")
    })?;
    Ok(ts.as_millisecond())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_argument_parser_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_subcommand_means_the_application() {
        let cli = Cli::parse_from(["toolog"]);
        assert!(cli.command.is_none(), "a bare invocation opens the app");
        assert!(!cli.background);
    }

    #[test]
    fn the_login_agent_passes_background() {
        let cli = Cli::parse_from(["toolog", "--background"]);
        assert!(cli.background);
        assert!(cli.command.is_none());
    }

    #[test]
    fn doctor_fix_is_opt_in() {
        assert!(matches!(
            Cli::parse_from(["toolog", "doctor"]).command,
            Some(Command::Doctor { fix: false })
        ));
        assert!(matches!(
            Cli::parse_from(["toolog", "doctor", "--fix"]).command,
            Some(Command::Doctor { fix: true })
        ));
    }

    #[test]
    fn relative_times_are_accepted_alongside_timestamps() {
        let now = jiff::Timestamp::now().as_millisecond();
        let day = parse_since("1d").expect("1d");
        assert!((now - day - 86_400_000).abs() < 5_000, "about a day ago");

        assert_eq!(
            parse_since("2026-01-01T00:00:00Z").expect("timestamp"),
            1_767_225_600_000
        );
        assert!(parse_since("last tuesday").is_err());
    }

    #[test]
    fn export_defaults_to_json_over_stdout() {
        let cli = Cli::parse_from(["toolog", "export"]);
        let Some(Command::Export(args)) = cli.command else {
            panic!("expected export");
        };
        assert_eq!(args.format, Format::Json);
        assert!(args.output.is_none());
        assert!(!args.rejected);
    }

    #[test]
    fn export_filters_parse_the_way_they_read() {
        let cli = Cli::parse_from([
            "toolog",
            "export",
            "--format",
            "csv",
            "--tool",
            "Bash",
            "--rejected",
            "--limit",
            "10",
        ]);
        let Some(Command::Export(args)) = cli.command else {
            panic!("expected export");
        };
        assert_eq!(args.format, Format::Csv);
        assert_eq!(args.tool.as_deref(), Some("Bash"));
        assert!(args.rejected);
        assert_eq!(args.limit, Some(10));
    }
}
