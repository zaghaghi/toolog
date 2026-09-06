//! Pointing toolog at a model, and reporting what it found (tasks 13.1–13.3).
//!
//! **toolog never downloads anything.** [ADR-0008] is the tool's central claim
//! and `tests/egress.rs` in this crate enforces it structurally: adding an HTTP
//! client to the workspace fails the build before it can be written. A 3.1 GB
//! fetch from Hugging Face is exactly the thing that test exists to forbid. So
//! the user brings the file, in their own shell, on their own network, and
//! toolog is given a path.
//!
//! This module is what stands between that path and llama.cpp. A user who points
//! at a 3 GB file that turns out to be a tarball is told so, in a sentence,
//! before any C++ opens it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use toolog_llm::gguf;

/// The model this documentation names, and the one command that fetches it.
///
/// Written out here rather than left to a link, because the Status card shows it
/// and a user should not have to guess which quantization is meant. **toolog
/// never runs this.** It is a line to paste into a shell.
pub const SUGGESTED_REPO: &str = "google/gemma-4-E2B-it-qat-q4_0-gguf";
/// The file within that repository.
pub const SUGGESTED_FILE: &str = "gemma-4-E2B_q4_0-it.gguf";

/// The `curl` line, for the Status card and for `toolog model status`.
#[must_use]
pub fn fetch_command() -> String {
    format!(
        "curl -L -o {SUGGESTED_FILE} \\\n  \
         https://huggingface.co/{SUGGESTED_REPO}/resolve/main/{SUGGESTED_FILE}"
    )
}

/// What the window shows about the examination, and what the CLI prints.
///
/// Lives here rather than in `analysis.rs` because that module needs the
/// `inference` feature and this type does not: a build without an engine still
/// has to be able to say that there is no examination running.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct AnalysisStatus {
    /// Whether a model is loaded at all.
    pub running: bool,
    /// Whether the backfill is working or deliberately stopped.
    pub paused: bool,
    /// Verdicts recorded since this process started — not since the beginning
    /// of time, which is what [`toolog_core::llm::Progress`] is for.
    pub done_this_run: i64,
    /// Answers the schema rejected since this process started (task 13.10).
    pub failed_this_run: i64,
    /// Arriving calls dropped because the model was still busy.
    ///
    /// Surfaced rather than swallowed: it is the honest cost of not blocking
    /// ingestion, and the backfill picks them up regardless.
    pub skipped_live: i64,
    /// The last thing that went wrong, if anything has.
    pub last_error: Option<String>,
}

/// What the window and the CLI show about the configured model.
///
/// One type for both, so `toolog model status` and the Status card cannot tell
/// different stories about the same file — the same rule the doctor report
/// follows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct ModelStatus {
    /// Whether this build can run a model at all (task 13.19's fallback).
    ///
    /// "No model configured" and "this build cannot load one" are different
    /// answers, and a card that showed the first while meaning the second would
    /// send someone looking for a file that would not help.
    pub supported: bool,
    /// The configured path, whether or not it is any good.
    pub path: Option<String>,
    /// What the file turned out to be, when it is a model.
    pub file: Option<gguf::ModelFile>,
    /// Why the configured path is not usable, in a sentence.
    pub problem: Option<String>,
    /// Whether a model is loaded and answering right now.
    pub loaded: bool,
    /// The repository and file this documentation suggests.
    pub suggested: String,
    /// The command that fetches it. **Never run by toolog.**
    pub fetch_command: String,
}

impl ModelStatus {
    /// The state of a machine where nobody has configured anything.
    #[must_use]
    pub fn unconfigured() -> Self {
        Self {
            supported: toolog_llm::built_with_inference(),
            path: None,
            file: None,
            problem: None,
            loaded: false,
            suggested: format!("{SUGGESTED_REPO} → {SUGGESTED_FILE}"),
            fetch_command: fetch_command(),
        }
    }

    /// Whether the configured path names a usable model.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        self.supported && self.file.is_some() && self.problem.is_none()
    }
}

/// Inspect a configured path without loading it.
///
/// Reads the GGUF header only — a few hundred kilobytes, not three gigabytes —
/// so the Status card can be drawn on every open without hashing the file.
#[must_use]
pub fn status(path: Option<&Path>, loaded: bool) -> ModelStatus {
    let mut status = ModelStatus {
        loaded,
        ..ModelStatus::unconfigured()
    };
    let Some(path) = path else {
        return status;
    };
    status.path = Some(path.display().to_string());

    if !status.supported {
        status.problem = Some(
            "this build was compiled without inference support, so it cannot load a model"
                .to_string(),
        );
        return status;
    }

    match gguf::inspect(path) {
        Ok(file) => status.file = Some(file),
        Err(e) => status.problem = Some(e.to_string()),
    }
    status
}

/// Check a path and hash it, for the moment someone chooses a file.
///
/// The hash is the model's identity (task 13.14) and reading 3.1 GB takes a
/// second or two, so it happens here — once, when the file is chosen — rather
/// than on every status read.
pub fn adopt(path: &Path) -> Result<gguf::ModelFile, gguf::GgufError> {
    toolog_llm::inspect_model(path)
}

/// Expand a leading `~` and make the path absolute.
///
/// A path typed into a terminal is very often `~/models/gemma.gguf`, and a path
/// stored in `prefs.json` has to survive the process's working directory
/// changing — which for a resident menu-bar app it will.
#[must_use]
pub fn normalize(path: &str, home: &Path) -> PathBuf {
    let trimmed = path.trim();
    let expanded = if trimmed == "~" {
        home.to_path_buf()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&expanded))
            .unwrap_or(expanded)
    }
}

/// The lines `toolog model status` prints.
#[must_use]
pub fn render(status: &ModelStatus) -> String {
    let mut out = String::new();
    out.push_str("Local model (Phase 13 — a second opinion)\n\n");

    if !status.supported {
        out.push_str("  This build has no inference support compiled in.\n\n");
    }

    match (&status.path, &status.file, &status.problem) {
        (None, _, _) => {
            out.push_str("  No model configured. Nothing is analysed, and nothing has changed.\n");
        }
        (Some(path), Some(file), _) => {
            let _ = writeln!(out, "  path     {path}");
            let _ = writeln!(out, "  model    {}", file.describe());
            if let Some(name) = &file.name {
                let _ = writeln!(out, "  name     {name}");
            }
            if let Some(sha) = &file.sha256 {
                let _ = writeln!(out, "  sha256   {sha}");
            }
            let _ = writeln!(
                out,
                "  state    {}",
                if status.loaded {
                    "loaded"
                } else {
                    "not loaded"
                }
            );
        }
        (Some(path), None, problem) => {
            let _ = writeln!(out, "  path     {path}");
            let _ = writeln!(
                out,
                "  problem  {}",
                problem.as_deref().unwrap_or("unreadable")
            );
        }
    }

    if status.file.is_none() {
        let _ = writeln!(
            out,
            "\n  The model this is written for:\n    {}\n\n  \
             Fetch it yourself — toolog has no network capability and never will:\n\n    {}",
            status.suggested,
            status.fetch_command.replace('\n', "\n    ")
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_configured_reads_as_nothing_configured() {
        let status = status(None, false);
        assert!(status.path.is_none());
        assert!(status.problem.is_none(), "absence is not a problem");
        assert!(!status.is_usable());
        assert!(render(&status).contains("No model configured"));
    }

    #[test]
    fn a_path_that_is_not_a_model_reports_the_sentence_rather_than_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"this is not a model").expect("write");

        let status = status(Some(&path), false);
        assert!(status.file.is_none());
        assert!(!status.is_usable());
        let problem = status.problem.expect("a problem");
        assert!(problem.contains("not a GGUF model"), "{problem}");
    }

    #[test]
    fn a_missing_file_is_reported_rather_than_silently_ignored() {
        let status = status(Some(Path::new("/nowhere/gemma.gguf")), false);
        assert!(status.problem.is_some());
        assert!(render(&status).contains("problem"));
    }

    /// The one thing this module promises: the command is shown, never run.
    #[test]
    fn the_fetch_command_is_shown_and_names_the_quantization() {
        let command = fetch_command();
        assert!(command.starts_with("curl -L -o "));
        assert!(command.contains(SUGGESTED_REPO));
        assert!(command.contains(SUGGESTED_FILE));
        assert!(
            render(&status(None, false)).contains("toolog has no network capability"),
            "the report has to say whose network this is"
        );
    }

    #[test]
    fn a_tilde_path_is_expanded_against_the_home_directory() {
        let home = Path::new("/Users/someone");
        assert_eq!(
            normalize("~/models/gemma.gguf", home),
            PathBuf::from("/Users/someone/models/gemma.gguf")
        );
        assert_eq!(normalize("~", home), PathBuf::from("/Users/someone"));
        assert_eq!(
            normalize("  /abs/gemma.gguf  ", home),
            PathBuf::from("/abs/gemma.gguf"),
            "and surrounding whitespace from a paste is not part of the path"
        );
        assert!(
            normalize("relative.gguf", home).is_absolute(),
            "a stored path must survive the working directory changing"
        );
    }
}
