//! The resident process's half of the second opinion (Phase 13).
//!
//! Loading a model is slow — 1.5 seconds to hash 3.1 GB, then half a second to
//! load it — and none of that may happen on the setup path, or a menu-bar app
//! takes two seconds to appear because of a preference. So this is a handle that
//! is **empty until it is not**: the window asks it what is going on, the live
//! sink offers it calls, and a background thread fills it in.
//!
//! It is shared by three things that must not know about each other: the live
//! sink (which exists before [`crate::state::AppState`] does), the command
//! surface, and the thread doing the loading. Hence an `Arc<Llm>` created first
//! and handed to each.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[cfg(feature = "inference")]
use toolog_cli::analysis::Analysis;
use toolog_cli::model::{AnalysisStatus, ModelStatus};
use toolog_core::llm::Pair;
use toolog_core::writer::WriteHandle;

/// What the window is told about the model and the examination.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub(crate) struct LlmReport {
    /// The configured file, and what it turned out to be.
    pub(crate) model: ModelStatus,
    /// Whether a load is in flight. A card that said "no model" during those two
    /// seconds would be answering a different question from the one asked.
    pub(crate) starting: bool,
    /// Why the last load failed, if it did.
    pub(crate) error: Option<String>,
    /// The running examination, when there is one.
    pub(crate) analysis: Option<AnalysisStatus>,
    /// How far it has got over the whole store.
    pub(crate) progress: Option<toolog_core::llm::Progress>,
    /// Which model and prompt these numbers are about, short form.
    pub(crate) pair: Option<String>,
    /// The prompt template's own version, stated in words (task 13.16).
    pub(crate) prompt_fingerprint: String,
    /// How many examined calls fell at each score, worst first.
    pub(crate) scores: Vec<toolog_core::llm::ScoreTally>,
    /// The highest-scoring commands no rule matched.
    pub(crate) worst: Vec<toolog_core::llm::Scored>,
}

/// How many of the worst commands the risk view shows before "and the rest".
const WORST_SHOWN: u32 = 20;
/// The lowest score worth putting in that list.
const WORST_FROM: i64 = 4;

#[derive(Default)]
struct Inner {
    #[cfg(feature = "inference")]
    analysis: Option<Arc<Analysis>>,
    /// Kept even after the model is unloaded, so `@llm-risk` still means
    /// something for verdicts already recorded.
    pair: Option<Pair>,
    starting: bool,
    error: Option<String>,
}

/// The shared handle.
#[derive(Debug, Default)]
pub(crate) struct Llm {
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(feature = "inference")]
        let loaded = self.analysis.is_some();
        #[cfg(not(feature = "inference"))]
        let loaded = false;
        f.debug_struct("Inner")
            .field("loaded", &loaded)
            .field("starting", &self.starting)
            .finish_non_exhaustive()
    }
}

impl Llm {
    /// The running examination, if a model is loaded.
    #[cfg(feature = "inference")]
    pub(crate) fn analysis(&self) -> Option<Arc<Analysis>> {
        self.inner.lock().ok().and_then(|i| i.analysis.clone())
    }

    /// Which (model, prompt) pair the stored verdicts belong to.
    pub(crate) fn pair(&self) -> Option<Pair> {
        self.inner.lock().ok().and_then(|i| i.pair.clone())
    }

    /// Offer an arriving call to the model (task 13.8). Cheap when there is none.
    #[cfg(feature = "inference")]
    pub(crate) fn observe(&self, tool_use_id: &str, command: &str) {
        if let Some(analysis) = self.analysis() {
            analysis.observe(tool_use_id, command);
        }
    }

    /// Without an engine there is nothing to offer a call to.
    #[cfg(not(feature = "inference"))]
    pub(crate) fn observe(&self, _tool_use_id: &str, _command: &str) {}

    /// Pause or resume the backfill (task 13.7).
    #[cfg(feature = "inference")]
    pub(crate) fn set_paused(&self, paused: bool) {
        if let Some(analysis) = self.analysis() {
            analysis.set_paused(paused);
        }
    }

    #[cfg(not(feature = "inference"))]
    pub(crate) fn set_paused(&self, _paused: bool) {}

    /// Stop the examination and put the model down. Idempotent.
    #[cfg(feature = "inference")]
    pub(crate) fn stop(&self) {
        let taken = self.inner.lock().ok().and_then(|mut i| i.analysis.take());
        if let Some(analysis) = taken {
            analysis.stop();
        }
    }

    #[cfg(not(feature = "inference"))]
    pub(crate) fn stop(&self) {}

    /// Load a model in the background and start examining.
    ///
    /// Returns immediately. Hashing 3.1 GB and loading it is about two seconds,
    /// and the window must not wait for either — so the report says `starting`
    /// until it is done, and says why if it fails.
    #[cfg(feature = "inference")]
    pub(crate) fn start(
        self: &Arc<Self>,
        db_path: PathBuf,
        model_path: PathBuf,
        writer: WriteHandle,
        paused: bool,
    ) {
        self.stop();
        {
            let Ok(mut inner) = self.inner.lock() else {
                return;
            };
            inner.starting = true;
            inner.error = None;
        }

        let me = Arc::clone(self);
        std::thread::Builder::new()
            .name("toolog-llm-load".into())
            .spawn(move || {
                // The hash is the model's identity (task 13.14) and reading it
                // is most of the wait, which is the reason this is a thread.
                let outcome = toolog_llm::gguf::sha256_file(&model_path)
                    .map_err(|e| e.to_string())
                    .and_then(|fingerprint| {
                        Analysis::start(db_path, model_path.clone(), &fingerprint, writer)
                            .map_err(|e| format!("{e:#}"))
                    });

                let Ok(mut inner) = me.inner.lock() else {
                    return;
                };
                inner.starting = false;
                match outcome {
                    Ok(analysis) => {
                        analysis.set_paused(paused);
                        inner.pair = Some(analysis.pair().clone());
                        inner.analysis = Some(Arc::new(analysis));
                        inner.error = None;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, path = %model_path.display(), "model not loaded");
                        inner.error = Some(e);
                    }
                }
            })
            .map_or_else(
                |e| tracing::error!(error = %e, "could not start the model loader"),
                |_| (),
            );
    }

    /// Without an engine, pointing at a model records the preference and does
    /// nothing else. The Status card says so, which is why `ModelStatus` carries
    /// `supported` — "no model configured" and "this build cannot load one" are
    /// different answers.
    #[cfg(not(feature = "inference"))]
    pub(crate) fn start(
        self: &Arc<Self>,
        _db_path: PathBuf,
        _model_path: PathBuf,
        _writer: WriteHandle,
        _paused: bool,
    ) {
    }

    /// Everything the Status card and the risk view's own section read.
    ///
    /// One command for both, so the two cannot describe the same model
    /// differently — the rule the doctor report and `RiskReview` already follow.
    pub(crate) fn report(
        &self,
        configured: Option<&std::path::Path>,
        read: impl FnOnce(&Pair) -> anyhow::Result<Numbers>,
    ) -> LlmReport {
        #[cfg(feature = "inference")]
        let (running, pair, starting, error) = match self.inner.lock() {
            Ok(inner) => (
                inner.analysis.as_ref().map(|a| a.status()),
                inner.pair.clone(),
                inner.starting,
                inner.error.clone(),
            ),
            Err(_) => (None, None, false, Some("the model lock is poisoned".into())),
        };
        #[cfg(not(feature = "inference"))]
        let (running, pair, starting, error): (
            Option<AnalysisStatus>,
            Option<Pair>,
            bool,
            Option<String>,
        ) = match self.inner.lock() {
            Ok(inner) => (None, inner.pair.clone(), false, inner.error.clone()),
            Err(_) => (None, None, false, Some("the model lock is poisoned".into())),
        };

        // A store read that fails is reported rather than swallowed: without
        // this the card would show "0 examined" for a connection that could not
        // be read at all, which is the wrong answer rather than no answer.
        let (numbers, read_error) = match pair.as_ref().map(read) {
            Some(Ok(numbers)) => (Some(numbers), None),
            Some(Err(e)) => (None, Some(format!("{e:#}"))),
            None => (None, None),
        };

        LlmReport {
            model: toolog_cli::model::status(configured, running.is_some()),
            starting,
            error: error.or(read_error),
            analysis: running,
            progress: numbers.as_ref().map(|n| n.progress),
            pair: pair.as_ref().map(Pair::short),
            prompt_fingerprint: toolog_llm::Prompt::current()
                .short_fingerprint()
                .to_string(),
            scores: numbers
                .as_ref()
                .map(|n| n.scores.clone())
                .unwrap_or_default(),
            worst: numbers.map(|n| n.worst).unwrap_or_default(),
        }
    }
}

/// What one read of the store supplies to a report.
pub(crate) struct Numbers {
    pub(crate) progress: toolog_core::llm::Progress,
    pub(crate) scores: Vec<toolog_core::llm::ScoreTally>,
    pub(crate) worst: Vec<toolog_core::llm::Scored>,
}

/// Read them, on whichever connection the caller has.
pub(crate) fn numbers(conn: &toolog_core::Connection, pair: &Pair) -> anyhow::Result<Numbers> {
    Ok(Numbers {
        progress: toolog_core::llm::progress(conn, pair)?,
        scores: toolog_core::llm::score_tallies(conn, pair)?,
        worst: toolog_core::llm::top_scoring(conn, pair, WORST_FROM, WORST_SHOWN)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The phase's first exit criterion, at the one place it can be asserted
    /// without a window: with nothing configured, the handle holds nothing,
    /// answers nothing, and does nothing when offered a call.
    ///
    /// The thread is the thing that must not exist, and the only way one is
    /// ever started is [`Llm::start`], which `AppState::apply_model` calls only
    /// when `prefs.model()` is `Some`. That is asserted from the other end by
    /// `toolog_cli::prefs`'s test that a default `Prefs` names no model.
    #[test]
    fn with_no_model_the_handle_is_empty_and_answers_nothing() {
        let llm = Llm::default();
        assert!(llm.pair().is_none());
        #[cfg(feature = "inference")]
        assert!(llm.analysis().is_none());

        // Offering a call and stopping are both no-ops rather than errors: the
        // live sink calls the first on every arriving call, and shutdown calls
        // the second whether or not anything was ever loaded.
        llm.observe("toolu_1", "ls -la");
        llm.set_paused(true);
        llm.stop();
        llm.stop();
    }

    /// A report with no model reads as "no model", not as "nothing found".
    #[test]
    fn a_report_with_no_model_carries_no_numbers_to_be_mistaken_for_zero() {
        let llm = Llm::default();
        let report = llm.report(None, |_| unreachable!("nothing to read for"));

        assert!(report.model.path.is_none());
        assert!(
            report.progress.is_none(),
            "not `0 of 0`, which reads as clean"
        );
        assert!(report.pair.is_none());
        assert!(report.scores.is_empty());
        assert!(report.worst.is_empty());
        assert!(!report.starting);
        assert!(report.error.is_none(), "absence is not an error");
        // The prompt has an identity whether or not a model exists to run it.
        assert_eq!(report.prompt_fingerprint.len(), 12);
    }

    /// A store read that fails is surfaced, not silently rendered as zero.
    #[test]
    fn a_failed_read_becomes_the_reports_error_rather_than_an_empty_result() {
        let llm = Llm::default();
        if let Ok(mut inner) = llm.inner.lock() {
            inner.pair = Some(Pair::new("model-a", "prompt-a"));
        }

        let report = llm.report(None, |_| Err(anyhow::anyhow!("the database is locked")));
        assert!(report.progress.is_none());
        assert!(
            report
                .error
                .as_deref()
                .is_some_and(|e| e.contains("locked")),
            "{:?}",
            report.error
        );
    }

    /// The pair outlives the model being unloaded, so `@llm-risk` still means
    /// something for verdicts already recorded.
    #[test]
    fn stopping_the_model_does_not_forget_which_question_the_verdicts_answered() {
        let llm = Llm::default();
        if let Ok(mut inner) = llm.inner.lock() {
            inner.pair = Some(Pair::new("model-a", "prompt-a"));
        }
        llm.stop();
        assert_eq!(llm.pair(), Some(Pair::new("model-a", "prompt-a")));
    }
}
