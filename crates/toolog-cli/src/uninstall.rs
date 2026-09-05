//! `toolog uninstall` — the reverse of everything the installer did (task 8.6).
//!
//! Phase 8 asks for this to be "as carefully built as the install", and the
//! reason is [ADR-0006]: `doctor --fix` writes to a file this application does
//! not own. Writing someone else's configuration is only defensible if putting
//! it back is a real operation rather than a paragraph in a readme.
//!
//! # Restoring, not deleting keys
//!
//! The exit criterion is that `~/.claude/settings.json` ends up **byte-
//! identical** to its pre-install state. Only one thing achieves that: writing
//! back the copy taken before the first `--fix`, which carries the user's own
//! formatting, key order and trailing whitespace. Removing our keys and
//! re-serializing would produce a semantically equal file that differs byte for
//! byte, and a diff nobody asked for is exactly what an uninstaller must not
//! leave behind.
//!
//! But a backup can only be restored safely if nothing else changed since. If
//! the user added a hook, an MCP server or a model preference after installing,
//! restoring the backup would silently throw that away — a far worse outcome
//! than an imperfect byte match.
//!
//! So the two are computed and **compared**:
//!
//! - Remove our keys from the file as it stands now.
//! - Read the oldest backup — the one taken before the first `--fix`.
//! - If the two agree as JSON, nothing but our keys ever changed, and the
//!   backup is restored byte for byte. This is the ordinary case.
//! - If they disagree, the file has moved on for reasons that are not ours.
//!   Only our keys are removed, everything else is kept, and the report says
//!   plainly which of the two happened and why.
//!
//! The user is told which path was taken either way. An uninstaller that
//! quietly picks one is not one you can check.
//!
//! [ADR-0006]: ../../../docs/adr/0006-configure-via-settings-env-block.md

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::settings::{self, FixError, Scope, Stack};
use crate::{launchagent, prefs};

/// What reverting `~/.claude/settings.json` would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsRevert {
    /// The file has none of our keys. Nothing to undo.
    Clean,
    /// The backup and a key-by-key removal agree: restore it byte for byte.
    RestoreBackup {
        backup: PathBuf,
        keys: Vec<&'static str>,
    },
    /// They disagree, or there is no backup. Remove our keys, keep the rest.
    RemoveKeys {
        keys: Vec<&'static str>,
        /// Why the byte-identical restore was not available.
        reason: String,
    },
    /// We created the file and it holds nothing else. Remove it.
    RemoveFile { keys: Vec<&'static str> },
    /// Set somewhere we never write, so not ours to undo.
    BeyondOurReach { scopes: Vec<Scope> },
    /// The file cannot be parsed, so it cannot be edited safely.
    Unreadable { message: String },
}

impl SettingsRevert {
    /// Whether applying this would change anything on disk.
    #[must_use]
    pub fn changes_anything(&self) -> bool {
        matches!(
            self,
            Self::RestoreBackup { .. } | Self::RemoveKeys { .. } | Self::RemoveFile { .. }
        )
    }
}

/// One thing the uninstall would delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub path: PathBuf,
    /// What it is, in the user's terms.
    pub what: &'static str,
    pub bytes: u64,
}

/// Everything `toolog uninstall` would do, computed without doing any of it.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The login agent, if one is installed.
    pub agent: Option<PathBuf>,
    pub settings_path: PathBuf,
    pub settings: SettingsRevert,
    /// The store, preferences, rules and logs. Kept unless asked for.
    pub data: Vec<Item>,
    pub data_dir: Option<PathBuf>,
    pub delete_data: bool,
    /// The `.app` we are running from, which cannot delete itself.
    pub app_bundle: Option<PathBuf>,
    /// The resident process is up, so capture is still running.
    pub running: bool,
}

impl Plan {
    /// Total size of what would be deleted.
    #[must_use]
    pub fn data_bytes(&self) -> u64 {
        self.data.iter().map(|i| i.bytes).sum()
    }

    /// Whether applying this would change anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        let deleting = self.delete_data && !self.data.is_empty();
        self.agent.is_none() && !self.settings.changes_anything() && !deleting
    }
}

/// Work out what an uninstall would do.
///
/// Reads only. `apply` is what acts on this.
#[must_use]
pub fn plan(home: &Path, cwd: &Path, delete_data: bool) -> Plan {
    let stack = Stack::read(cwd, home);
    let settings_path = settings::user_path(home);

    let agent = launchagent::status(home)
        .installed
        .then(|| launchagent::plist_path(home));

    let data = data_items();
    let data_dir = toolog_core::db::data_dir().ok();

    Plan {
        agent,
        settings: plan_settings(&stack, &settings_path),
        settings_path,
        data,
        data_dir,
        delete_data,
        app_bundle: app_bundle(),
        running: toolog_otlp::health::probe(toolog_otlp::port::default_addr()).is_up(),
    }
}

/// The keys `doctor --fix` writes, without needing to know the endpoint.
fn our_keys() -> Vec<&'static str> {
    settings::desired_env("http://127.0.0.1:0")
        .into_iter()
        .map(|(key, _)| key)
        .collect()
}

/// Decide how `~/.claude/settings.json` should be put back.
fn plan_settings(stack: &Stack, path: &Path) -> SettingsRevert {
    let user = stack.user();
    if let Some(message) = &user.error {
        return SettingsRevert::Unreadable {
            message: message.clone(),
        };
    }

    let present: Vec<&'static str> = our_keys()
        .into_iter()
        .filter(|key| user.env(key).is_some())
        .collect();

    if present.is_empty() {
        // Ours nowhere in the file we write — but possibly set in a file we
        // never write, which is worth saying rather than reporting "clean".
        let elsewhere: Vec<Scope> = our_keys()
            .into_iter()
            .flat_map(|key| stack.all_settings_of(key))
            .map(|(scope, _)| scope)
            .filter(|scope| *scope != Scope::User)
            .collect();
        let mut scopes: Vec<Scope> = elsewhere;
        scopes.sort_unstable();
        scopes.dedup();
        return if scopes.is_empty() {
            SettingsRevert::Clean
        } else {
            SettingsRevert::BeyondOurReach { scopes }
        };
    }

    let Some(Value::Object(current)) = user.json.clone() else {
        return SettingsRevert::Unreadable {
            message: "top level is not an object".to_string(),
        };
    };
    let stripped = without_our_keys(&current);

    // Nothing but our keys: the file is one we created, so the honest revert
    // is for it to stop existing — but only when no backup says otherwise.
    let backups = settings::backups_of(path);
    if stripped.is_empty() && backups.is_empty() {
        return SettingsRevert::RemoveFile { keys: present };
    }

    let Some(oldest) = backups.first() else {
        return SettingsRevert::RemoveKeys {
            keys: present,
            reason: format!(
                "no backup was taken beside {}, so there is no pre-install copy to restore",
                path.display()
            ),
        };
    };

    match std::fs::read_to_string(oldest)
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(original)) if original == stripped => SettingsRevert::RestoreBackup {
                backup: oldest.clone(),
                keys: present,
            },
            Ok(_) => SettingsRevert::RemoveKeys {
                keys: present,
                reason: format!(
                    "{} has changed since {} was taken, in ways that are not ours; \
                     restoring it would discard those edits",
                    path.display(),
                    oldest.display()
                ),
            },
            Err(e) => SettingsRevert::RemoveKeys {
                keys: present,
                reason: format!("{} is not readable as JSON ({e})", oldest.display()),
            },
        },
        None => SettingsRevert::RemoveKeys {
            keys: present,
            reason: format!("{} could not be read", oldest.display()),
        },
    }
}

/// A copy of `root` with our keys gone, and `env` gone if that empties it.
fn without_our_keys(root: &Map<String, Value>) -> Map<String, Value> {
    let mut out = root.clone();
    let Some(Value::Object(env)) = out.get_mut("env") else {
        return out;
    };
    for key in our_keys() {
        env.remove(key);
    }
    if env.is_empty() {
        out.remove("env");
    }
    out
}

/// The files an uninstall would delete, when asked to.
fn data_items() -> Vec<Item> {
    let Ok(dir) = toolog_core::db::data_dir() else {
        return Vec::new();
    };
    let db = dir.join("toolog.db");

    // Named individually rather than as one directory, because `rules.toml` is
    // something the user may have written themselves and should see listed.
    let candidates: Vec<(PathBuf, &'static str)> = vec![
        (db.clone(), "the database: every tool call recorded"),
        (db.with_extension("db-wal"), "the write-ahead log"),
        (db.with_extension("db-shm"), "the shared-memory index"),
        (dir.join("prefs.json"), "your preferences"),
        (
            dir.join("rules.toml"),
            "your own risk rules, if you wrote any",
        ),
        (dir.join("logs"), "this application's own logs"),
    ];

    candidates
        .into_iter()
        .filter(|(path, _)| path.exists())
        .map(|(path, what)| Item {
            bytes: size_of_path(&path),
            path,
            what,
        })
        .collect()
}

/// Bytes on disk, following one level of directory.
fn size_of_path(path: &Path) -> u64 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// The `.app` this binary is running from, if it is running from one.
fn app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|a| a.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

/// What an applied uninstall actually did, step by step.
#[derive(Debug, Default)]
pub struct Outcome {
    pub done: Vec<String>,
    pub failed: Vec<String>,
}

/// Carry out a plan.
///
/// Every step is independent and a failure in one does not stop the others: a
/// half-uninstalled tool that says which half is better than one that stops at
/// the first error and leaves the user guessing.
#[must_use]
pub fn apply(home: &Path, plan: &Plan) -> Outcome {
    let mut out = Outcome::default();

    if let Some(path) = &plan.agent {
        match launchagent::uninstall(home) {
            Ok(()) => out
                .done
                .push(format!("Removed the login agent {}", path.display())),
            Err(e) => out
                .failed
                .push(format!("Could not remove {}: {e}", path.display())),
        }
    }

    match apply_settings(&plan.settings, &plan.settings_path) {
        Ok(Some(line)) => out.done.push(line),
        Ok(None) => {}
        Err(e) => out
            .failed
            .push(format!("{}: {e}", plan.settings_path.display())),
    }

    if plan.delete_data {
        for item in &plan.data {
            let result = if item.path.is_dir() {
                std::fs::remove_dir_all(&item.path)
            } else {
                std::fs::remove_file(&item.path)
            };
            match result {
                Ok(()) => out.done.push(format!("Deleted {}", item.path.display())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => out
                    .failed
                    .push(format!("Could not delete {}: {e}", item.path.display())),
            }
        }
        // Only if we emptied it. A directory with something else in it is
        // something else's, whatever the path says.
        if let Some(dir) = &plan.data_dir
            && std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none())
            && std::fs::remove_dir(dir).is_ok()
        {
            out.done.push(format!("Removed {}", dir.display()));
        }
    }

    out
}

/// Put `~/.claude/settings.json` back, and say in one line what was done.
fn apply_settings(revert: &SettingsRevert, path: &Path) -> Result<Option<String>, FixError> {
    match revert {
        SettingsRevert::Clean
        | SettingsRevert::BeyondOurReach { .. }
        | SettingsRevert::Unreadable { .. } => Ok(None),

        SettingsRevert::RestoreBackup { backup, .. } => {
            settings::restore_backup(backup, path)?;
            Ok(Some(format!(
                "Restored {} from {}",
                path.display(),
                backup.display()
            )))
        }

        SettingsRevert::RemoveFile { .. } => {
            std::fs::remove_file(path).map_err(|source| FixError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            Ok(Some(format!(
                "Removed {} — it held nothing but our keys",
                path.display()
            )))
        }

        SettingsRevert::RemoveKeys { keys, .. } => {
            let text = std::fs::read_to_string(path).map_err(|source| FixError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let Ok(Value::Object(root)) = serde_json::from_str::<Value>(&text) else {
                return Err(FixError::Unparseable {
                    path: path.to_path_buf(),
                    message: "not a JSON object".to_string(),
                });
            };
            // A backup here too: this path rewrites a file we could not prove
            // we understood, which is exactly when a copy is worth having.
            let backup = settings::backup_file(path)?;
            let mut out = serde_json::to_string_pretty(&Value::Object(without_our_keys(&root)))
                .unwrap_or_else(|e| unreachable!("serializing a JSON object cannot fail: {e}"));
            out.push('\n');
            settings::write_atomically(path, out.as_bytes())?;
            Ok(Some(format!(
                "Removed {} key(s) from {}; the rest of the file is untouched (backup: {})",
                keys.len(),
                path.display(),
                backup.display()
            )))
        }
    }
}

/// Whether preferences exist that the user would lose.
#[must_use]
pub fn has_preferences() -> bool {
    prefs::path().is_some_and(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Install into a temp home the way `doctor --fix` would, then plan a
    /// revert against it.
    fn install(home: &Path, before: Option<&str>) -> PathBuf {
        let path = settings::user_path(home);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        if let Some(text) = before {
            std::fs::write(&path, text).expect("seed");
        }
        let stack = Stack::read(home, home);
        settings::apply_fix(&stack, "http://127.0.0.1:47318").expect("fix");
        path
    }

    fn revert_of(home: &Path) -> SettingsRevert {
        let stack = Stack::read(home, home);
        plan_settings(&stack, &settings::user_path(home))
    }

    /// The exit criterion of task 8.6, as an assertion.
    #[test]
    fn uninstall_leaves_settings_json_byte_identical_to_its_pre_install_state() {
        let home = tempfile::tempdir().expect("tempdir");
        // Deliberately idiosyncratic: tabs, a trailing comment-free blank line,
        // and a key order no serializer would reproduce.
        let original = "{\n\t\"model\": \"opus\",\n\t\"env\": {\n\t\t\"FOO\": \"bar\"\n\t}\n}\n";
        let path = install(home.path(), Some(original));

        assert_ne!(
            std::fs::read_to_string(&path).expect("read"),
            original,
            "the fix must actually have changed the file, or this proves nothing"
        );

        let revert = revert_of(home.path());
        let SettingsRevert::RestoreBackup { .. } = &revert else {
            panic!("expected a byte-identical restore, got {revert:?}");
        };
        apply_settings(&revert, &path).expect("apply");

        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            original,
            "byte-identical, tabs and key order included"
        );
    }

    #[test]
    fn a_file_we_created_is_removed_rather_than_left_empty() {
        let home = tempfile::tempdir().expect("tempdir");
        let path = install(home.path(), None);
        assert!(path.exists());

        let revert = revert_of(home.path());
        assert!(
            matches!(revert, SettingsRevert::RemoveFile { .. }),
            "nothing but our keys were ever in it: {revert:?}"
        );
        apply_settings(&revert, &path).expect("apply");
        assert!(!path.exists(), "an empty {{}} left behind is litter");
    }

    /// The case the backup must not win: the user changed something of theirs.
    #[test]
    fn edits_made_after_installing_are_kept_and_the_backup_is_not_restored() {
        let home = tempfile::tempdir().expect("tempdir");
        let path = install(home.path(), Some("{\"model\": \"opus\"}\n"));

        // The user adds a hook after installing.
        let mut root: Map<String, Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        root.insert("hooks".into(), serde_json::json!({"PreToolUse": []}));
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&Value::Object(root)).expect("write"),
        )
        .expect("write");

        let revert = revert_of(home.path());
        let SettingsRevert::RemoveKeys { reason, .. } = &revert else {
            panic!("restoring the backup would have deleted the hook: {revert:?}");
        };
        assert!(reason.contains("has changed since"), "{reason}");

        apply_settings(&revert, &path).expect("apply");
        let after: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert!(after.get("hooks").is_some(), "the user's hook survives");
        assert_eq!(after.get("model").and_then(Value::as_str), Some("opus"));
        assert!(
            after.get("env").is_none(),
            "our keys were the only env keys, so the block goes with them"
        );
    }

    #[test]
    fn a_foreign_env_key_keeps_the_env_block_alive() {
        let home = tempfile::tempdir().expect("tempdir");
        let path = install(home.path(), Some("{\"env\": {\"HTTP_PROXY\": \"x\"}}\n"));

        // Force the RemoveKeys path by making the backup unusable.
        for backup in settings::backups_of(&path) {
            std::fs::write(&backup, "not json").expect("clobber");
        }
        let revert = revert_of(home.path());
        apply_settings(&revert, &path).expect("apply");

        let after: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(
            after.pointer("/env/HTTP_PROXY").and_then(Value::as_str),
            Some("x"),
            "someone else's variable is not ours to remove"
        );
        for (key, _) in settings::desired_env("http://127.0.0.1:0") {
            assert!(after.pointer(&format!("/env/{key}")).is_none(), "{key}");
        }
    }

    #[test]
    fn an_uninstalled_machine_reports_nothing_to_do() {
        let home = tempfile::tempdir().expect("tempdir");
        assert_eq!(revert_of(home.path()), SettingsRevert::Clean);
        assert!(!revert_of(home.path()).changes_anything());
    }

    #[test]
    fn keys_set_by_a_file_we_never_write_are_reported_rather_than_touched() {
        let home = tempfile::tempdir().expect("tempdir");
        let cwd = home.path().join("work");
        let project = cwd.join(".claude");
        std::fs::create_dir_all(&project).expect("mkdir");
        std::fs::write(
            project.join("settings.json"),
            r#"{"env": {"CLAUDE_CODE_ENABLE_TELEMETRY": "1"}}"#,
        )
        .expect("seed");

        let stack = Stack::read(&cwd, home.path());
        let revert = plan_settings(&stack, &settings::user_path(home.path()));
        let SettingsRevert::BeyondOurReach { scopes } = &revert else {
            panic!("expected a report, not an edit: {revert:?}");
        };
        assert_eq!(scopes, &[Scope::Project]);
        assert!(!revert.changes_anything(), "we do not write project files");
    }

    #[test]
    fn every_key_the_installer_writes_is_a_key_the_uninstaller_removes() {
        let written: Vec<&str> = settings::desired_env("http://127.0.0.1:47318")
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            our_keys(),
            written,
            "a key added to desired_env and not removed here would survive an uninstall"
        );
    }
}
