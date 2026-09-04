//! `toolog doctor` — the install experience, and most of the future support
//! burden.
//!
//! Two questions have to be answerable in one command: *is Claude Code
//! configured to send me anything?* and *am I actually receiving it?* They fail
//! independently and for unrelated reasons, so the report separates them rather
//! than collapsing both into one green tick.
//!
//! Read-only. Nothing here writes a file; [`fix`] does, and only when asked.
//!
//! Two failure modes get explicit treatment because they otherwise look like
//! the tool being broken:
//!
//! - **A managed policy overrides the user file.** Enterprise settings win over
//!   anything we write, so `doctor` names the file instead of reporting a
//!   configuration that is not in force.
//! - **Something else holds the port.** A bound port is not our receiver, so
//!   health is probed, not inferred.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use toolog_core::constants::{DEFAULT_OTLP_HOST, DEFAULT_OTLP_PORT};
use toolog_core::{Db, query};
use toolog_otlp::health::{self, Health};

use crate::launchagent;
use crate::settings::{self, Scope, Stack};

/// One environment variable, as configured versus as required.
#[derive(Debug, Clone)]
pub struct EnvCheck {
    pub key: &'static str,
    pub want: String,
    /// The value that actually applies, and which file supplies it.
    pub have: Option<(Scope, String)>,
}

impl EnvCheck {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.have.as_ref().is_some_and(|(_, v)| *v == self.want)
    }

    /// Set, but not by the file we would write — so `--fix` cannot correct it.
    #[must_use]
    pub fn beyond_our_reach(&self) -> bool {
        self.have
            .as_ref()
            .is_some_and(|(scope, _)| *scope != Scope::User)
    }
}

/// What we can see of `~/.claude/projects`.
#[derive(Debug, Clone)]
pub struct TranscriptCheck {
    pub dir: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub files: usize,
    pub bytes: u64,
    /// Files with at least one record already stored.
    pub ingested_files: i64,
}

/// What we can see of our own store.
#[derive(Debug, Clone)]
pub struct DatabaseCheck {
    pub path: PathBuf,
    pub exists: bool,
    pub bytes: u64,
    pub error: Option<String>,
    pub totals: Option<query::Totals>,
    pub ingest: Option<query::IngestSummary>,
}

/// The whole picture, gathered in one pass.
#[derive(Debug, Clone)]
pub struct Report {
    /// The endpoint `--fix` would write, without the `/v1/logs` path.
    pub endpoint: String,
    pub probe_addr: SocketAddr,
    pub health: Health,
    pub env: Vec<EnvCheck>,
    /// The managed policy file, when it sets telemetry variables of its own.
    pub managed: Option<PathBuf>,
    pub settings_path: PathBuf,
    pub settings_error: Option<String>,
    pub transcripts: TranscriptCheck,
    pub database: DatabaseCheck,
    pub agent: launchagent::Status,
}

impl Report {
    /// Whether Claude Code is configured to send us logs.
    #[must_use]
    pub fn configured(&self) -> bool {
        self.env.iter().all(EnvCheck::ok)
    }

    /// Everything wrong, in the order a user should deal with it.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();

        if let Some(path) = &self.managed {
            out.push(format!(
                "A managed policy at {} sets telemetry variables. It takes precedence over \
                 ~/.claude/settings.json, so `doctor --fix` cannot override it.",
                path.display()
            ));
        }
        if let Some(e) = &self.settings_error {
            out.push(format!(
                "{} could not be read: {e}",
                self.settings_path.display()
            ));
        }

        let unreachable: Vec<&str> = self
            .env
            .iter()
            .filter(|c| !c.ok() && c.beyond_our_reach())
            .map(|c| c.key)
            .collect();
        if !unreachable.is_empty() {
            out.push(format!(
                "Set outside ~/.claude/settings.json and therefore not ours to correct: {}",
                unreachable.join(", ")
            ));
        }

        if !self.configured() {
            out.push(
                "Claude Code is not configured to export logs here. Run `toolog doctor --fix`."
                    .to_string(),
            );
        }

        match &self.health {
            Health::Up(_) => {}
            Health::Down => out.push(format!(
                "Nothing is listening on {}. Start toolog, or install the login agent.",
                self.probe_addr
            )),
            Health::Foreign(what) => out.push(format!(
                "{} is held by something that is not toolog ({what}). \
                 The receiver will fall back to the next free port and rewrite the endpoint.",
                self.probe_addr
            )),
        }

        if !self.transcripts.exists {
            out.push(format!(
                "{} does not exist, so the content half of the audit trail is unavailable.",
                self.transcripts.dir.display()
            ));
        } else if !self.transcripts.readable {
            out.push(format!(
                "{} is not readable.",
                self.transcripts.dir.display()
            ));
        }

        if let Some(e) = &self.database.error {
            out.push(format!("The database could not be opened: {e}"));
        }

        out
    }
}

/// Where to look. Overridable so the whole report is testable.
#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub db: PathBuf,
}

impl Paths {
    /// The real locations for this machine.
    pub fn detect() -> toolog_core::Result<Self> {
        Ok(Self {
            home: settings::home_dir(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            db: toolog_core::db::default_path()?,
        })
    }
}

/// Gather the report.
#[must_use]
pub fn report(paths: &Paths) -> Report {
    let stack = Stack::read(&paths.cwd, &paths.home);
    let probe_addr = resolve_addr(&stack);
    let endpoint = format!("http://{probe_addr}");

    let env = settings::desired_env(&endpoint)
        .into_iter()
        .map(|(key, want)| EnvCheck {
            key,
            want,
            have: stack
                .effective(key)
                .map(|(scope, value)| (scope, value.to_string())),
        })
        .collect();

    let managed = stack
        .files
        .iter()
        .find(|f| {
            f.scope == Scope::Managed
                && settings::desired_env(&endpoint)
                    .iter()
                    .any(|(k, _)| f.env(k).is_some())
        })
        .map(|f| f.path.clone());

    Report {
        health: health::probe(probe_addr),
        probe_addr,
        endpoint,
        env,
        managed,
        settings_path: stack.user().path.clone(),
        settings_error: stack.user().error.clone(),
        transcripts: check_transcripts(&paths.home, &paths.db),
        database: check_database(&paths.db),
        agent: launchagent::status(&paths.home),
    }
}

/// The address to report on, and to write into Claude Code's configuration.
///
/// Prefers a receiver that is actually answering — including on a fallback port
/// from an earlier conflict — over the default, so `doctor` describes the
/// running system rather than the intended one.
fn resolve_addr(stack: &Stack) -> SocketAddr {
    let default = SocketAddr::new(
        DEFAULT_OTLP_HOST
            .parse()
            .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
        DEFAULT_OTLP_PORT,
    );
    if health::probe(default).is_up() {
        return default;
    }
    if let Some((_, configured)) = stack.effective(settings::LOGS_ENDPOINT_KEY)
        && let Some(addr) = parse_authority(configured)
        && addr.ip().is_loopback()
        && health::probe(addr).is_up()
    {
        return addr;
    }
    default
}

/// `http://host:port/whatever` to a socket address, when it is one.
fn parse_authority(url: &str) -> Option<SocketAddr> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    authority.parse().ok()
}

fn check_transcripts(home: &Path, db: &Path) -> TranscriptCheck {
    let dir = settings::projects_dir(home);
    let exists = dir.is_dir();
    let readable = exists && std::fs::read_dir(&dir).is_ok();

    let files = if readable {
        toolog_ingest::discover::transcripts(&dir)
    } else {
        Vec::new()
    };
    let bytes = files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();

    TranscriptCheck {
        dir,
        exists,
        readable,
        files: files.len(),
        bytes,
        ingested_files: open_readable(db)
            .and_then(|db| query::ingest_summary(db.conn()).ok())
            .map_or(0, |s| s.transcript_files),
    }
}

fn check_database(path: &Path) -> DatabaseCheck {
    let exists = path.is_file();
    let bytes = std::fs::metadata(path).map_or(0, |m| m.len());

    if !exists {
        return DatabaseCheck {
            path: path.to_path_buf(),
            exists,
            bytes,
            error: None,
            totals: None,
            ingest: None,
        };
    }

    match Db::open(path) {
        Ok(db) => DatabaseCheck {
            path: path.to_path_buf(),
            exists,
            bytes,
            error: None,
            totals: query::stats_totals(db.conn()).ok(),
            ingest: query::ingest_summary(db.conn()).ok(),
        },
        Err(e) => DatabaseCheck {
            path: path.to_path_buf(),
            exists,
            bytes,
            error: Some(e.to_string()),
            totals: None,
            ingest: None,
        },
    }
}

/// Open the database only if it already exists.
///
/// `doctor` reports; it does not create the store as a side effect of being
/// asked a question.
fn open_readable(path: &Path) -> Option<Db> {
    path.is_file().then(|| Db::open(path).ok())?
}

/// Apply the ADR-0006 `env` block.
///
/// The endpoint comes from the same resolution the report uses, so `--fix`
/// writes the port that is actually in service.
pub fn fix(paths: &Paths) -> Result<settings::Applied, settings::FixError> {
    let stack = Stack::read(&paths.cwd, &paths.home);
    let endpoint = format!("http://{}", resolve_addr(&stack));
    settings::apply_fix(&stack, &endpoint)
}

/// Render the report for a terminal.
#[must_use]
pub fn render(report: &Report) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    // Three states, not two: "not yet" is not a failure, and marking it as one
    // trains the reader to ignore the column.
    let mark = |ok: bool| if ok { "  ok " } else { "FAIL " };
    let note = "  -- ";

    let _ = writeln!(out, "Claude Code configuration");
    let _ = writeln!(out, "  file: {}", report.settings_path.display());
    for check in &report.env {
        let value = match &check.have {
            Some((scope, v)) if *scope == Scope::User => v.clone(),
            Some((scope, v)) => format!("{v}   [from {}]", scope.label()),
            None => "not set".to_string(),
        };
        let _ = writeln!(out, "  {}{:<34} {value}", mark(check.ok()), check.key);
    }

    let _ = writeln!(out, "\nReceiver");
    match &report.health {
        Health::Up(c) => {
            let _ = writeln!(out, "  {}listening on {}", mark(true), report.probe_addr);
            let _ = writeln!(
                out,
                "       {} batches, {} records, {} dropped, {} rejected",
                c.batches, c.records, c.dropped, c.rejected_bodies
            );
        }
        Health::Down => {
            let _ = writeln!(
                out,
                "  {}nothing listening on {}",
                mark(false),
                report.probe_addr
            );
        }
        Health::Foreign(what) => {
            let _ = writeln!(
                out,
                "  {}{} is held by something else ({what})",
                mark(false),
                report.probe_addr
            );
        }
    }

    let _ = writeln!(out, "\nTranscripts");
    let t = &report.transcripts;
    let _ = writeln!(out, "  {}{}", mark(t.exists && t.readable), t.dir.display());
    if t.readable {
        let _ = writeln!(
            out,
            "       {} files, {}, {} already ingested",
            t.files,
            human_bytes(t.bytes),
            t.ingested_files
        );
    }

    let _ = writeln!(out, "\nStore");
    let d = &report.database;
    let store_mark = match (d.exists, &d.error) {
        (true, None) => mark(true),
        (false, _) => note,
        (true, Some(_)) => mark(false),
    };
    let _ = writeln!(out, "  {store_mark}{}", d.path.display());
    if let Some(totals) = &d.totals {
        let _ = writeln!(
            out,
            "       {}, {} tool calls in {} sessions, {} raw records",
            human_bytes(d.bytes),
            totals.tool_calls,
            totals.sessions,
            totals.raw_events
        );
    } else if !d.exists {
        let _ = writeln!(out, "       not created yet — run `toolog backfill`");
    }

    let _ = writeln!(out, "\nLogin agent");
    let a = &report.agent;
    if a.supported {
        let state = match (a.installed, a.loaded) {
            (false, _) => "not installed — capture stops when toolog is not running".to_string(),
            (true, Some(true)) => format!("installed and loaded ({})", a.path.display()),
            (true, Some(false)) => format!("installed but not loaded ({})", a.path.display()),
            (true, None) => format!("installed ({})", a.path.display()),
        };
        // Not installing the agent is a supported choice, not a fault.
        let _ = writeln!(
            out,
            "  {}{state}",
            if a.installed { mark(true) } else { note }
        );
    } else {
        let _ = writeln!(out, "  {note}not applicable on this platform");
    }

    let problems = report.problems();
    if problems.is_empty() {
        let _ = writeln!(out, "\nEverything checks out.");
    } else {
        let _ = writeln!(out, "\nWhat to do");
        for (i, p) in problems.iter().enumerate() {
            let _ = writeln!(out, "  {}. {p}", i + 1);
        }
    }

    out
}

fn human_bytes(bytes: u64) -> String {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a display figure; a byte of drift in a MiB reading is not a defect"
    )]
    let b = bytes as f64;
    for (limit, unit) in [
        (1024.0_f64.powi(3), "GiB"),
        (1024.0_f64.powi(2), "MiB"),
        (1024.0, "KiB"),
    ] {
        if b >= limit {
            return format!("{:.1} {unit}", b / limit);
        }
    }
    format!("{bytes} B")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths_in(dir: &Path) -> Paths {
        Paths {
            home: dir.to_path_buf(),
            cwd: dir.to_path_buf(),
            db: dir.join("toolog.db"),
        }
    }

    #[test]
    fn a_clean_machine_reports_telemetry_off_and_says_what_to_do() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = report(&paths_in(dir.path()));

        assert!(!report.configured(), "nothing is configured yet");
        assert!(report.env.iter().all(|c| c.have.is_none()));
        let problems = report.problems().join("\n");
        assert!(
            problems.contains("toolog doctor --fix"),
            "the report must name the command that fixes it: {problems}"
        );

        let text = render(&report);
        assert!(text.contains("CLAUDE_CODE_ENABLE_TELEMETRY"));
        assert!(text.contains("not set"));
    }

    #[test]
    fn a_fixed_machine_reports_itself_configured() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(dir.path());
        std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");

        fix(&paths).expect("fix");
        let report = report(&paths);

        assert!(report.configured(), "{:?}", report.env);
        assert!(
            !report.problems().iter().any(|p| p.contains("--fix")),
            "nothing left to fix: {:?}",
            report.problems()
        );
    }

    /// The failure mode that otherwise looks like the tool being broken.
    #[test]
    fn a_setting_from_a_file_we_cannot_write_is_named_as_such() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(dir.path());
        std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
        std::fs::write(
            dir.path().join(".claude").join("settings.json"),
            r#"{"env": {"CLAUDE_CODE_ENABLE_TELEMETRY": "0"}}"#,
        )
        .expect("write project settings");

        let report = report(&paths);
        let check = report
            .env
            .iter()
            .find(|c| c.key == "CLAUDE_CODE_ENABLE_TELEMETRY")
            .expect("the check exists");
        assert!(!check.ok());
        assert!(
            check.beyond_our_reach(),
            "a project file outranks the user file"
        );

        let problems = report.problems().join("\n");
        assert!(
            problems.contains("not ours to correct"),
            "must not claim a fix it cannot perform: {problems}"
        );
    }

    #[test]
    fn a_missing_database_is_reported_not_created() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = paths_in(dir.path());
        let report = report(&paths);

        assert!(!report.database.exists);
        assert!(
            !paths.db.exists(),
            "doctor is read-only; asking a question must not create a store"
        );
        assert!(render(&report).contains("not created yet"));
    }

    #[test]
    fn transcripts_are_counted_when_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let projects = dir.path().join(".claude").join("projects").join("-x");
        std::fs::create_dir_all(&projects).expect("mkdir");
        std::fs::write(projects.join("a.jsonl"), "{}\n").expect("write");
        std::fs::write(projects.join("b.jsonl"), "{}\n").expect("write");

        let report = report(&paths_in(dir.path()));
        assert!(report.transcripts.exists && report.transcripts.readable);
        assert_eq!(report.transcripts.files, 2);
    }

    #[test]
    fn byte_sizes_read_the_way_a_person_would_say_them() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MiB");
    }
}
