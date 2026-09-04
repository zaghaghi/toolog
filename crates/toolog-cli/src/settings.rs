//! Reading and writing Claude Code's `settings.json` ([ADR-0006]).
//!
//! This module writes to a file the application does not own, so every rule in
//! ADR-0006 is enforced here rather than left to the caller:
//!
//! 1. **Per-signal variables only.** `OTEL_EXPORTER_OTLP_ENDPOINT` is global
//!    across signals; setting it would silently redirect a user's metrics and
//!    traces — possibly their employer's — to a local process. It is never
//!    written, and [`FORBIDDEN_KEYS`] is asserted against the desired set in a
//!    test so it cannot be added by accident.
//! 2. **Merge, never overwrite.** Only our keys change; every other key, and
//!    the order of all of them, survives.
//! 3. **Atomic write, timestamped backup.** Temp file plus rename, with the
//!    original copied first so the uninstall path is a real revert.
//! 4. **Abort on a foreign endpoint.** If a non-loopback OTLP logs endpoint is
//!    already configured, stop and say so. Taking over someone's telemetry
//!    pipeline silently is exactly the failure this tool must not have.
//!
//! [ADR-0006]: ../../../docs/adr/0006-configure-via-settings-env-block.md

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Where a setting came from. Ordered highest-precedence first, which is the
/// order [`Stack::files`] holds them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// Enterprise/managed policy. Wins over everything, including us.
    Managed,
    /// `.claude/settings.local.json` in the working directory.
    ProjectLocal,
    /// `.claude/settings.json` in the working directory.
    Project,
    /// `~/.claude/settings.json` — the only file we ever write.
    User,
}

impl Scope {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Managed => "managed (enterprise policy)",
            Self::ProjectLocal => "project (settings.local.json)",
            Self::Project => "project (settings.json)",
            Self::User => "user (~/.claude/settings.json)",
        }
    }
}

/// One settings file, read or absent.
#[derive(Debug, Clone)]
pub struct SettingsFile {
    pub scope: Scope,
    pub path: PathBuf,
    /// Parsed contents. `None` if the file is absent or unparseable.
    pub json: Option<Value>,
    /// Why it could not be read, when that is the reason `json` is `None`.
    pub error: Option<String>,
}

impl SettingsFile {
    /// Whether the file is present on disk.
    #[must_use]
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// One value from this file's `env` block.
    #[must_use]
    pub fn env(&self, key: &str) -> Option<&str> {
        self.json.as_ref()?.get("env")?.get(key)?.as_str()
    }
}

/// The settings files that apply, in precedence order.
#[derive(Debug, Clone)]
pub struct Stack {
    pub files: Vec<SettingsFile>,
}

impl Stack {
    /// Read the stack that applies when Claude Code runs in `cwd`.
    #[must_use]
    pub fn read(cwd: &Path, home: &Path) -> Self {
        let candidates = [
            (Scope::Managed, managed_path()),
            (
                Scope::ProjectLocal,
                cwd.join(".claude").join("settings.local.json"),
            ),
            (Scope::Project, cwd.join(".claude").join("settings.json")),
            (Scope::User, user_path(home)),
        ];

        let files = candidates
            .into_iter()
            .map(|(scope, path)| match std::fs::read_to_string(&path) {
                Ok(text) => match serde_json::from_str::<Value>(&text) {
                    Ok(json) => SettingsFile {
                        scope,
                        path,
                        json: Some(json),
                        error: None,
                    },
                    Err(e) => SettingsFile {
                        scope,
                        path,
                        json: None,
                        error: Some(format!("invalid JSON: {e}")),
                    },
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => SettingsFile {
                    scope,
                    path,
                    json: None,
                    error: None,
                },
                Err(e) => SettingsFile {
                    scope,
                    path,
                    json: None,
                    error: Some(e.to_string()),
                },
            })
            .collect();

        Self { files }
    }

    /// Read the stack for the current directory and home.
    #[must_use]
    pub fn read_default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::read(&cwd, &home_dir())
    }

    /// The file we write: `~/.claude/settings.json`.
    ///
    /// # Panics
    ///
    /// Never: [`Stack::read`] always produces a [`Scope::User`] entry.
    #[must_use]
    pub fn user(&self) -> &SettingsFile {
        self.files
            .iter()
            .find(|f| f.scope == Scope::User)
            .expect("the user scope is always present")
    }

    /// The effective value of an `env` key, and which file supplies it.
    #[must_use]
    pub fn effective(&self, key: &str) -> Option<(Scope, &str)> {
        self.files
            .iter()
            .find_map(|f| f.env(key).map(|v| (f.scope, v)))
    }

    /// Every scope that sets `key`, highest precedence first.
    #[must_use]
    pub fn all_settings_of(&self, key: &str) -> Vec<(Scope, &str)> {
        self.files
            .iter()
            .filter_map(|f| f.env(key).map(|v| (f.scope, v)))
            .collect()
    }

    /// Whether a managed policy overrides the file we would write.
    #[must_use]
    pub fn managed_overrides(&self, key: &str) -> bool {
        self.files
            .iter()
            .any(|f| f.scope == Scope::Managed && f.env(key).is_some())
    }
}

/// `~/.claude/settings.json`.
#[must_use]
pub fn user_path(home: &Path) -> PathBuf {
    home.join(".claude").join("settings.json")
}

/// The enterprise policy file for this platform.
#[must_use]
pub fn managed_path() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json")
    } else if cfg!(target_os = "windows") {
        PathBuf::from(r"C:\ProgramData\ClaudeCode\managed-settings.json")
    } else {
        PathBuf::from("/etc/claude-code/managed-settings.json")
    }
}

/// The user's home directory, falling back to `.` rather than failing.
#[must_use]
pub fn home_dir() -> PathBuf {
    directories::UserDirs::new().map_or_else(|| PathBuf::from("."), |d| d.home_dir().to_path_buf())
}

/// `~/.claude/projects`, where transcripts live.
#[must_use]
pub fn projects_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("projects")
}

/// Variables this tool must never write, checked in a test against the set it
/// does write.
///
/// `OTEL_EXPORTER_OTLP_ENDPOINT` is global across signals. The other two turn
/// on prompt and response capture, which stays off by default (ADR-0008).
pub const FORBIDDEN_KEYS: &[&str] = &[
    "OTEL_EXPORTER_OTLP_ENDPOINT",
    "OTEL_EXPORTER_OTLP_PROTOCOL",
    "OTEL_LOG_USER_PROMPTS",
    "OTEL_LOG_ASSISTANT_RESPONSES",
];

/// The key whose value decides where Claude Code sends its logs.
pub const LOGS_ENDPOINT_KEY: &str = "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT";

/// The `env` block ADR-0006 specifies, for a receiver at `endpoint`.
///
/// `endpoint` is a base URL such as `http://127.0.0.1:47318`; the `/v1/logs`
/// path is appended here because a **per-signal** OTLP endpoint is used as-is
/// by the exporter, with nothing appended. Phase 3 lost an end-to-end test to
/// exactly this: without the path the exporter posts to `/`, gets a 404, and
/// reports it only at debug level.
#[must_use]
pub fn desired_env(endpoint: &str) -> Vec<(&'static str, String)> {
    vec![
        ("CLAUDE_CODE_ENABLE_TELEMETRY", "1".to_string()),
        ("OTEL_LOGS_EXPORTER", "otlp".to_string()),
        (
            "OTEL_EXPORTER_OTLP_LOGS_PROTOCOL",
            "http/protobuf".to_string(),
        ),
        (LOGS_ENDPOINT_KEY, logs_url(endpoint)),
        ("OTEL_LOGS_EXPORT_INTERVAL", "2000".to_string()),
        ("OTEL_LOG_TOOL_DETAILS", "1".to_string()),
    ]
}

/// Append `/v1/logs` to a base endpoint, idempotently.
#[must_use]
pub fn logs_url(endpoint: &str) -> String {
    let trimmed = endpoint.trim_end_matches('/');
    if trimmed.ends_with("/v1/logs") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1/logs")
    }
}

/// Whether a URL points at this machine.
///
/// Anything we cannot parse is treated as **not** loopback: the conservative
/// answer, since the consequence of being wrong is hijacking someone's
/// telemetry.
#[must_use]
pub fn is_loopback_url(url: &str) -> bool {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    // Strip userinfo, then take the host out of `host:port` or `[v6]:port`.
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };

    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// Why a `--fix` must not proceed.
#[derive(Debug, thiserror::Error)]
pub enum FixError {
    /// ADR-0006 rule 4. The user has a telemetry pipeline; we do not take it.
    #[error(
        "{scope} already sends Claude Code logs to {endpoint}, which is not on this machine.\n\
         toolog will not redirect an existing telemetry pipeline. Remove or change \
         {key} in {path} first, then re-run."
    )]
    ForeignEndpoint {
        scope: &'static str,
        key: &'static str,
        endpoint: String,
        path: PathBuf,
    },
    /// The file exists but is not JSON we can safely rewrite.
    #[error("{path} could not be parsed ({message}); refusing to rewrite it")]
    Unparseable { path: PathBuf, message: String },
    /// The `env` key exists but is not an object.
    #[error("{path} has an \"env\" key that is not an object; refusing to rewrite it")]
    EnvNotAnObject { path: PathBuf },
    #[error("writing {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// What a `--fix` would change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FixPlan {
    /// Keys not currently present.
    pub added: Vec<(String, String)>,
    /// Keys present with a different value: `(key, from, to)`.
    pub changed: Vec<(String, String, String)>,
    /// Keys already correct.
    pub unchanged: Vec<String>,
}

impl FixPlan {
    /// Whether anything would be written.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty()
    }
}

/// Work out what `--fix` would do, and refuse if it must not.
///
/// Checks the **whole stack** for a foreign endpoint, not just the file we
/// write: a project-level or managed setting we would be unable to override is
/// still someone else's pipeline.
pub fn plan_fix(stack: &Stack, endpoint: &str) -> Result<FixPlan, FixError> {
    for (scope, value) in stack.all_settings_of(LOGS_ENDPOINT_KEY) {
        if !is_loopback_url(value) {
            let path = stack
                .files
                .iter()
                .find(|f| f.scope == scope)
                .map_or_else(PathBuf::new, |f| f.path.clone());
            return Err(FixError::ForeignEndpoint {
                scope: scope.label(),
                key: LOGS_ENDPOINT_KEY,
                endpoint: value.to_string(),
                path,
            });
        }
    }
    // The global variable is the one that would hijack metrics and traces too.
    for (scope, value) in stack.all_settings_of("OTEL_EXPORTER_OTLP_ENDPOINT") {
        if !is_loopback_url(value) {
            let path = stack
                .files
                .iter()
                .find(|f| f.scope == scope)
                .map_or_else(PathBuf::new, |f| f.path.clone());
            return Err(FixError::ForeignEndpoint {
                scope: scope.label(),
                key: "OTEL_EXPORTER_OTLP_ENDPOINT",
                endpoint: value.to_string(),
                path,
            });
        }
    }

    let user = stack.user();
    if let Some(message) = &user.error {
        return Err(FixError::Unparseable {
            path: user.path.clone(),
            message: message.clone(),
        });
    }

    let mut plan = FixPlan::default();
    for (key, want) in desired_env(endpoint) {
        match user.env(key) {
            Some(have) if have == want => plan.unchanged.push(key.to_string()),
            Some(have) => plan.changed.push((key.to_string(), have.to_string(), want)),
            None => plan.added.push((key.to_string(), want)),
        }
    }
    Ok(plan)
}

/// What an applied fix did.
#[derive(Debug, Clone)]
pub struct Applied {
    pub path: PathBuf,
    /// The copy taken before writing, if the file already existed.
    pub backup: Option<PathBuf>,
    pub plan: FixPlan,
}

/// Merge the ADR-0006 `env` block into `~/.claude/settings.json`.
///
/// Backs up, writes atomically, and touches nothing but our own keys.
pub fn apply_fix(stack: &Stack, endpoint: &str) -> Result<Applied, FixError> {
    let plan = plan_fix(stack, endpoint)?;
    let user = stack.user();

    if plan.is_noop() {
        return Ok(Applied {
            path: user.path.clone(),
            backup: None,
            plan,
        });
    }

    let mut root = match user.json.clone() {
        Some(Value::Object(map)) => map,
        Some(_) => {
            return Err(FixError::Unparseable {
                path: user.path.clone(),
                message: "top level is not an object".to_string(),
            });
        }
        None => Map::new(),
    };

    let Value::Object(env) = root
        .entry("env".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
    else {
        return Err(FixError::EnvNotAnObject {
            path: user.path.clone(),
        });
    };
    for (key, value) in desired_env(endpoint) {
        env.insert(key.to_string(), Value::String(value));
    }

    let backup = if user.exists() {
        Some(backup_file(&user.path)?)
    } else {
        None
    };

    let mut text = serde_json::to_string_pretty(&Value::Object(root))
        .unwrap_or_else(|e| unreachable!("serializing a JSON object cannot fail: {e}"));
    text.push('\n');
    write_atomically(&user.path, text.as_bytes())?;

    Ok(Applied {
        path: user.path.clone(),
        backup,
        plan,
    })
}

/// Copy `path` beside itself with a timestamp, and return the copy.
pub fn backup_file(path: &Path) -> Result<PathBuf, FixError> {
    let stamp = jiff::Timestamp::now()
        .strftime("%Y%m%dT%H%M%SZ")
        .to_string();
    let name = path.file_name().map_or_else(
        || "settings.json".to_string(),
        |n| n.to_string_lossy().into(),
    );
    let backup = path.with_file_name(format!("{name}.toolog-backup-{stamp}"));
    std::fs::copy(path, &backup).map_err(|source| FixError::Io {
        path: backup.clone(),
        source,
    })?;
    Ok(backup)
}

/// Restore a backup over the file it was taken from.
///
/// The uninstall path (Phase 8), and the thing that makes writing someone
/// else's config defensible.
pub fn restore_backup(backup: &Path, target: &Path) -> Result<(), FixError> {
    let bytes = std::fs::read(backup).map_err(|source| FixError::Io {
        path: backup.to_path_buf(),
        source,
    })?;
    write_atomically(target, &bytes)
}

/// Every backup this tool has left beside `path`, oldest first.
#[must_use]
pub fn backups_of(path: &Path) -> Vec<PathBuf> {
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    let prefix = format!("{name}.toolog-backup-");

    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: BTreeSet<PathBuf> = BTreeSet::new();
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with(&prefix))
        {
            found.insert(entry.path());
        }
    }
    // Names are timestamp-suffixed, so lexical order is chronological.
    found.into_iter().collect()
}

/// Write `bytes` to `path` via a temporary file in the same directory.
///
/// A rename within one filesystem is atomic, so a reader either sees the old
/// file or the new one — never a half-written config.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), FixError> {
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| FixError::Io { path, source }
    };

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io(dir))?;
    }

    let tmp = path.with_extension(format!("toolog-tmp-{}", std::process::id()));
    let mut file = std::fs::File::create(&tmp).map_err(io(&tmp))?;
    file.write_all(bytes).map_err(io(&tmp))?;
    file.sync_all().map_err(io(&tmp))?;
    drop(file);

    std::fs::rename(&tmp, path).map_err(io(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a stack whose user file is `text` (or absent) inside a temp home.
    fn stack_with(home: &Path, text: Option<&str>) -> Stack {
        let path = user_path(home);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        if let Some(text) = text {
            std::fs::write(&path, text).expect("write");
        }
        // A temp cwd keeps project-scoped files out of the picture.
        Stack::read(home, home)
    }

    #[test]
    fn the_global_endpoint_variable_is_never_written() {
        let written: Vec<&str> = desired_env("http://127.0.0.1:47318")
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        for forbidden in FORBIDDEN_KEYS {
            assert!(
                !written.contains(forbidden),
                "{forbidden} would hijack signals we were not asked to carry"
            );
        }
    }

    #[test]
    fn the_logs_endpoint_carries_the_path() {
        let env = desired_env("http://127.0.0.1:47318");
        let (_, endpoint) = env
            .iter()
            .find(|(k, _)| *k == LOGS_ENDPOINT_KEY)
            .expect("logs endpoint");
        assert_eq!(
            endpoint, "http://127.0.0.1:47318/v1/logs",
            "a per-signal endpoint is used as-is; without the path nothing is captured"
        );
        assert_eq!(logs_url("http://127.0.0.1:47318/v1/logs/"), *endpoint);
    }

    #[test]
    fn loopback_detection_is_conservative() {
        for yes in [
            "http://127.0.0.1:47318",
            "http://localhost:4318/v1/logs",
            "http://[::1]:47318",
            "https://127.0.0.1",
        ] {
            assert!(is_loopback_url(yes), "{yes} is this machine");
        }
        for no in [
            "http://otel.corp.example:4318",
            "http://10.0.0.5:4318/v1/logs",
            "http://user@collector:4318",
            "not a url",
            "",
        ] {
            assert!(!is_loopback_url(no), "{no} must not be treated as local");
        }
    }

    #[test]
    fn a_fix_merges_and_leaves_every_other_key_alone() {
        let home = tempfile::tempdir().expect("tempdir");
        let original = r#"{
  "model": "opus",
  "env": { "MY_VAR": "keep me" },
  "permissions": { "allow": ["Bash(ls:*)"] }
}"#;
        let stack = stack_with(home.path(), Some(original));

        let applied = apply_fix(&stack, "http://127.0.0.1:47318").expect("fix");
        let after: Value =
            serde_json::from_str(&std::fs::read_to_string(&applied.path).expect("read"))
                .expect("parse");

        assert_eq!(after["model"], "opus", "unrelated keys survive");
        assert_eq!(after["permissions"]["allow"][0], "Bash(ls:*)");
        assert_eq!(after["env"]["MY_VAR"], "keep me", "other env keys survive");
        assert_eq!(after["env"]["CLAUDE_CODE_ENABLE_TELEMETRY"], "1");
        assert_eq!(
            after["env"][LOGS_ENDPOINT_KEY],
            "http://127.0.0.1:47318/v1/logs"
        );
        assert!(
            after["env"].get("OTEL_LOG_USER_PROMPTS").is_none(),
            "content capture stays off by default (ADR-0008)"
        );
    }

    /// The exit criterion for Phase 4, stated as a test.
    #[test]
    fn the_backup_restores_the_file_byte_for_byte() {
        let home = tempfile::tempdir().expect("tempdir");
        // Deliberately idiosyncratic formatting: tabs, trailing spaces, no
        // final newline. A re-serialized file would not match.
        let original = "{\n\t\"model\":\t\"opus\",   \n\t\"env\": {\"A\":\"1\"}\n}";
        let stack = stack_with(home.path(), Some(original));
        let before = std::fs::read(user_path(home.path())).expect("read");

        let applied = apply_fix(&stack, "http://127.0.0.1:47318").expect("fix");
        let backup = applied.backup.expect("a backup was taken");
        assert_ne!(
            std::fs::read(&applied.path).expect("read"),
            before,
            "the fix did change the file"
        );

        restore_backup(&backup, &applied.path).expect("restore");
        assert_eq!(
            std::fs::read(&applied.path).expect("read"),
            before,
            "restoring the backup must be byte-identical"
        );
    }

    #[test]
    fn a_missing_file_is_created_with_only_our_keys() {
        let home = tempfile::tempdir().expect("tempdir");
        let stack = stack_with(home.path(), None);

        let applied = apply_fix(&stack, "http://127.0.0.1:47318").expect("fix");
        assert!(applied.backup.is_none(), "nothing existed to back up");

        let after: Value =
            serde_json::from_str(&std::fs::read_to_string(&applied.path).expect("read"))
                .expect("parse");
        let env = after["env"].as_object().expect("env object");
        assert_eq!(env.len(), desired_env("x").len());
        assert_eq!(after.as_object().expect("root").len(), 1, "only env");
    }

    /// ADR-0006 rule 4: someone else's pipeline is not ours to redirect.
    #[test]
    fn an_existing_remote_endpoint_aborts_the_fix() {
        let home = tempfile::tempdir().expect("tempdir");
        let stack = stack_with(
            home.path(),
            Some(
                r#"{"env": {"OTEL_EXPORTER_OTLP_LOGS_ENDPOINT": "http://otel.corp:4318/v1/logs"}}"#,
            ),
        );

        let err = plan_fix(&stack, "http://127.0.0.1:47318").expect_err("must refuse");
        assert!(matches!(err, FixError::ForeignEndpoint { .. }));
        assert!(
            err.to_string().contains("otel.corp"),
            "the message names the endpoint it refused to replace: {err}"
        );
    }

    #[test]
    fn an_existing_global_endpoint_also_aborts() {
        let home = tempfile::tempdir().expect("tempdir");
        let stack = stack_with(
            home.path(),
            Some(r#"{"env": {"OTEL_EXPORTER_OTLP_ENDPOINT": "http://otel.corp:4318"}}"#),
        );
        assert!(matches!(
            plan_fix(&stack, "http://127.0.0.1:47318"),
            Err(FixError::ForeignEndpoint { .. })
        ));
    }

    #[test]
    fn a_loopback_endpoint_from_an_earlier_run_is_not_foreign() {
        let home = tempfile::tempdir().expect("tempdir");
        let stack = stack_with(
            home.path(),
            Some(
                r#"{"env": {"OTEL_EXPORTER_OTLP_LOGS_ENDPOINT": "http://127.0.0.1:47319/v1/logs"}}"#,
            ),
        );
        let plan = plan_fix(&stack, "http://127.0.0.1:47318").expect("our own port is replaceable");
        assert!(
            plan.changed.iter().any(|(k, _, _)| k == LOGS_ENDPOINT_KEY),
            "a port change is a change, not an abort: {plan:?}"
        );
    }

    #[test]
    fn a_second_fix_is_a_no_op() {
        let home = tempfile::tempdir().expect("tempdir");
        let stack = stack_with(home.path(), Some("{}"));
        apply_fix(&stack, "http://127.0.0.1:47318").expect("first fix");

        let stack = Stack::read(home.path(), home.path());
        let plan = plan_fix(&stack, "http://127.0.0.1:47318").expect("plan");
        assert!(plan.is_noop(), "nothing left to change: {plan:?}");
        assert_eq!(plan.unchanged.len(), desired_env("x").len());

        let before = std::fs::read(user_path(home.path())).expect("read");
        let applied = apply_fix(&stack, "http://127.0.0.1:47318").expect("second fix");
        assert!(applied.backup.is_none(), "a no-op takes no backup");
        assert_eq!(std::fs::read(user_path(home.path())).expect("read"), before);
    }

    #[test]
    fn a_broken_settings_file_is_refused_rather_than_rewritten() {
        let home = tempfile::tempdir().expect("tempdir");
        let broken = "{ this is not json";
        let stack = stack_with(home.path(), Some(broken));

        assert!(matches!(
            plan_fix(&stack, "http://127.0.0.1:47318"),
            Err(FixError::Unparseable { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(user_path(home.path())).expect("read"),
            broken,
            "a file we could not parse is left exactly as it was"
        );
    }

    #[test]
    fn precedence_reports_the_file_that_actually_wins() {
        let home = tempfile::tempdir().expect("tempdir");
        let cwd = home.path().join("work");
        std::fs::create_dir_all(cwd.join(".claude")).expect("mkdir");
        std::fs::create_dir_all(home.path().join(".claude")).expect("mkdir");
        std::fs::write(
            user_path(home.path()),
            r#"{"env": {"CLAUDE_CODE_ENABLE_TELEMETRY": "1"}}"#,
        )
        .expect("write user");
        std::fs::write(
            cwd.join(".claude").join("settings.json"),
            r#"{"env": {"CLAUDE_CODE_ENABLE_TELEMETRY": "0"}}"#,
        )
        .expect("write project");

        let stack = Stack::read(&cwd, home.path());
        assert_eq!(
            stack.effective("CLAUDE_CODE_ENABLE_TELEMETRY"),
            Some((Scope::Project, "0")),
            "the project file wins over the user file"
        );
        assert_eq!(
            stack.all_settings_of("CLAUDE_CODE_ENABLE_TELEMETRY").len(),
            2
        );
    }

    #[test]
    fn backups_are_listed_oldest_first() {
        let home = tempfile::tempdir().expect("tempdir");
        let path = user_path(home.path());
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, "{}").expect("write");
        for stamp in ["20260101T000000Z", "20260202T000000Z"] {
            std::fs::write(
                path.with_file_name(format!("settings.json.toolog-backup-{stamp}")),
                "{}",
            )
            .expect("write backup");
        }

        let found = backups_of(&path);
        assert_eq!(found.len(), 2);
        assert!(
            found[0].to_string_lossy().contains("20260101"),
            "oldest first: {found:?}"
        );
    }
}
