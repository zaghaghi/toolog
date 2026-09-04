//! Structured logging to a rotating local file.
//!
//! The log is diagnostics, not evidence — evidence goes to `raw_event`. It
//! records what the collector and tailer are doing so a user who asks "why is
//! nothing being captured?" has something to read, and so the tray's *Reveal
//! Logs* action has somewhere to point.
//!
//! It stays on this machine like everything else ([ADR-0008]), and it is
//! deliberately kept free of tool inputs and results: a debug log is the
//! easiest place for content to leak out of the store's own retention and
//! redaction rules.
//!
//! [ADR-0008]: ../../../docs/adr/0008-local-only-zero-egress.md

use std::path::PathBuf;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Days of logs to keep. Rotation is daily, so this is a two-week window.
const MAX_LOG_FILES: usize = 14;

/// Where log files are written: `<data dir>/logs`.
pub fn log_dir() -> toolog_core::Result<PathBuf> {
    Ok(toolog_core::db::data_dir()?.join("logs"))
}

/// Keeps the background log writer alive.
///
/// Dropping it flushes and stops the writer, so the resident process must hold
/// it for its whole lifetime.
#[derive(Debug)]
pub struct LogGuard {
    _worker: Option<tracing_appender::non_blocking::WorkerGuard>,
    /// Where the log is, for the tray action and `doctor`.
    pub dir: Option<PathBuf>,
}

fn filter(default: &str) -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default))
}

/// Terminal-only logging, for a one-shot CLI command.
///
/// Warnings and errors on stderr, so command output on stdout stays pipeable.
pub fn init_cli() {
    let _ = tracing_subscriber::registry()
        .with(filter("warn"))
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .without_time()
                .with_target(false),
        )
        .try_init();
}

/// File plus terminal logging, for the resident process.
///
/// Falls back to terminal-only if the log directory cannot be created, because
/// failing to open a log file is not a reason to refuse to capture.
#[must_use]
pub fn init_app() -> LogGuard {
    let dir = log_dir()
        .ok()
        .filter(|d| std::fs::create_dir_all(d).is_ok());

    let Some(dir) = dir else {
        init_cli();
        tracing::warn!("no writable log directory; logging to stderr only");
        return LogGuard {
            _worker: None,
            dir: None,
        };
    };

    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("toolog")
        .filename_suffix("log")
        .max_log_files(MAX_LOG_FILES)
        .build(&dir);

    match appender {
        Ok(appender) => {
            let (writer, worker) = tracing_appender::non_blocking(appender);
            let _ = tracing_subscriber::registry()
                .with(filter("info"))
                .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(writer)
                        .with_ansi(false),
                )
                .try_init();
            LogGuard {
                _worker: Some(worker),
                dir: Some(dir),
            }
        }
        Err(e) => {
            init_cli();
            tracing::warn!(error = %e, "could not open the log file; logging to stderr only");
            LogGuard {
                _worker: None,
                dir: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_live_under_the_application_data_directory() {
        let dir = log_dir().expect("log dir");
        assert!(dir.ends_with("logs"));
        assert!(
            dir.to_string_lossy()
                .contains(toolog_core::constants::APP_NAME)
        );
    }
}
