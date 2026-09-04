//! The macOS LaunchAgent that keeps capture resident ([ADR-0007]).
//!
//! The OTLP endpoint has to be listening whenever Claude Code runs, because
//! OTEL events are not replayable from disk the way transcripts are: a session
//! that ran while nothing was listening loses its decision and cost layer
//! permanently.
//!
//! # `KeepAlive` is a dictionary, not `true`
//!
//! ADR-0007 asks for two things that a bare `KeepAlive` cannot both satisfy:
//! restart the process if it dies, *and* let the user stop capture by quitting
//! from the tray. `KeepAlive = { SuccessfulExit: false }` gives exactly that —
//! a crash is restarted, a clean exit is respected. Quitting therefore means
//! what it says, which matters for a tool asking to be trusted.
//!
//! Installation is never silent: it happens from the tray or from an explicit
//! command, and the plist is written where the user can read it.
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

use std::path::{Path, PathBuf};
use std::process::Command;

use toolog_core::constants::BUNDLE_ID;

/// Where launchd looks for per-user agents.
#[must_use]
pub fn plist_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{BUNDLE_ID}.plist"))
}

/// Whether this platform has a LaunchAgent at all.
#[must_use]
pub fn is_supported() -> bool {
    cfg!(target_os = "macos")
}

/// The state of the agent as far as we can observe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub supported: bool,
    pub path: PathBuf,
    /// The plist exists on disk.
    pub installed: bool,
    /// launchd currently has the job. `None` if we could not ask.
    pub loaded: Option<bool>,
}

/// Read the agent's state.
#[must_use]
pub fn status(home: &Path) -> Status {
    let path = plist_path(home);
    let installed = path.is_file();
    Status {
        supported: is_supported(),
        loaded: if is_supported() && installed {
            Some(is_loaded())
        } else {
            None
        },
        path,
        installed,
    }
}

/// The plist text for an executable at `exe`.
///
/// `--background` is what tells the binary it was started by launchd: it comes
/// up resident with no window, rather than opening one at login.
#[must_use]
pub fn render(exe: &Path, log_dir: &Path) -> String {
    let exe = xml_escape(&exe.to_string_lossy());
    let out = xml_escape(&log_dir.join("launchd.out.log").to_string_lossy());
    let err = xml_escape(&log_dir.join("launchd.err.log").to_string_lossy());

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{BUNDLE_ID}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>--background</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <!-- Restart a crash, but respect a deliberate quit from the tray. -->
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>StandardOutPath</key>
    <string>{out}</string>
    <key>StandardErrorPath</key>
    <string>{err}</string>
</dict>
</plist>
"#
    )
}

/// Why installing or removing the agent failed.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("LaunchAgents are a macOS feature; nothing to install on this platform")]
    Unsupported,
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("launchctl {verb} failed: {message}")]
    Launchctl { verb: &'static str, message: String },
}

/// Write the plist and load it.
///
/// Idempotent: an existing job is unloaded first, so this doubles as "update
/// the agent to point at a new binary".
pub fn install(home: &Path, exe: &Path, log_dir: &Path) -> Result<PathBuf, AgentError> {
    if !is_supported() {
        return Err(AgentError::Unsupported);
    }
    let path = write_plist(home, exe, log_dir)?;

    // Ignore the unload: it fails when nothing is loaded, which is the normal
    // first-install case.
    let _ = bootout();
    bootstrap(&path)?;
    Ok(path)
}

/// Write the plist without asking launchd to load it.
///
/// Split out so the file-shaped half is testable on any platform.
pub fn write_plist(home: &Path, exe: &Path, log_dir: &Path) -> Result<PathBuf, AgentError> {
    let path = plist_path(home);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|source| AgentError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(&path, render(exe, log_dir)).map_err(|source| AgentError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Unload the job and delete the plist.
pub fn uninstall(home: &Path) -> Result<(), AgentError> {
    if is_supported() {
        let _ = bootout();
    }
    let path = plist_path(home);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(AgentError::Io { path, source }),
    }
}

fn domain() -> String {
    // SAFETY-free equivalent of getuid(): launchctl accepts the numeric uid,
    // and `id -u` is the portable way to obtain it without an unsafe libc call.
    let uid = Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "0".to_string(), |s| s.trim().to_string());
    format!("gui/{uid}")
}

fn bootstrap(path: &Path) -> Result<(), AgentError> {
    let output = Command::new("launchctl")
        .args(["bootstrap", &domain()])
        .arg(path)
        .output()
        .map_err(|e| AgentError::Launchctl {
            verb: "bootstrap",
            message: e.to_string(),
        })?;

    if output.status.success() {
        Ok(())
    } else {
        Err(AgentError::Launchctl {
            verb: "bootstrap",
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

fn bootout() -> Result<(), AgentError> {
    let output = Command::new("launchctl")
        .args(["bootout", &format!("{}/{BUNDLE_ID}", domain())])
        .output()
        .map_err(|e| AgentError::Launchctl {
            verb: "bootout",
            message: e.to_string(),
        })?;

    if output.status.success() {
        Ok(())
    } else {
        Err(AgentError::Launchctl {
            verb: "bootout",
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

fn is_loaded() -> bool {
    Command::new("launchctl")
        .args(["print", &format!("{}/{BUNDLE_ID}", domain())])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plist_restarts_a_crash_but_respects_a_quit() {
        let text = render(
            Path::new("/Applications/toolog.app/Contents/MacOS/toolog"),
            Path::new("/tmp/l"),
        );
        assert!(text.contains("<key>KeepAlive</key>"));
        assert!(
            text.contains("<key>SuccessfulExit</key>\n        <false/>"),
            "a bare KeepAlive would undo the tray's Quit: {text}"
        );
        assert!(
            text.contains("<string>--background</string>"),
            "no window at login"
        );
        assert!(text.contains(&format!("<string>{BUNDLE_ID}</string>")));
    }

    #[test]
    fn paths_with_xml_metacharacters_do_not_break_the_plist() {
        let text = render(Path::new("/Users/a&b/tool<og>"), Path::new("/tmp"));
        assert!(text.contains("/Users/a&amp;b/tool&lt;og&gt;"));
        assert!(
            !text.contains("a&b"),
            "unescaped ampersand would corrupt the plist"
        );
    }

    #[test]
    fn writing_and_removing_the_plist_is_idempotent() {
        let home = tempfile::tempdir().expect("tempdir");
        let path = write_plist(home.path(), Path::new("/bin/true"), home.path()).expect("write");
        assert!(path.is_file());
        assert!(status(home.path()).installed);

        write_plist(home.path(), Path::new("/bin/true"), home.path()).expect("rewrite");
        uninstall(home.path()).expect("uninstall");
        assert!(!path.exists());
        uninstall(home.path()).expect("uninstalling twice is not an error");
    }

    #[test]
    fn status_of_an_uninstalled_agent_says_so_without_guessing() {
        let home = tempfile::tempdir().expect("tempdir");
        let s = status(home.path());
        assert!(!s.installed);
        assert_eq!(s.loaded, None, "unknown is not the same as false");
    }
}
