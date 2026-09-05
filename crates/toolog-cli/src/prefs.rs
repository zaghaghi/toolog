//! What the user has turned on (task 6.12).
//!
//! Notifications are **off by default and individually toggleable**, which is
//! the whole requirement and the reason this file exists rather than a pair of
//! booleans in the window's `localStorage`: a preference the resident process
//! acts on has to be readable by the resident process, and it has to survive a
//! restart the same way the LaunchAgent does.
//!
//! `Default` is every switch off. A file that fails to parse is treated as
//! absent rather than as an error: a monitoring tool that will not start
//! because a preferences file has a typo in it is worse than one that starts
//! quiet.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The switches, all of them off until someone says otherwise.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(default)]
#[ts(export_to = "unused/")]
pub struct Prefs {
    /// Notify when a call is refused — by a person, a hook or a rule.
    pub notify_refusals: bool,
    /// Notify when a call trips a high-severity risk rule.
    pub notify_high_risk: bool,
    /// Redact secrets from the **evidence** as well as the projection.
    ///
    /// Off, like everything else here, and the trade-off is real in both
    /// directions — see [`toolog_core::redact`]. Off keeps secrets on disk in
    /// `raw_event` and keeps every projection rebuildable; on stops storing
    /// them at all and makes that irreversible. It is forward-only either way:
    /// turning it on does not reach back into records already stored.
    pub redact_evidence: bool,
    /// Projects never to capture, by path (task 7.8).
    ///
    /// Enforced at discovery, so an excluded project's transcript is never
    /// opened and nothing from it is ever stored — which is a different and
    /// stronger thing than filtering it out of a view.
    #[serde(default)]
    pub excluded_projects: Vec<String>,
}

impl Prefs {
    /// Whether any notification needs watching for.
    #[must_use]
    pub fn any(&self) -> bool {
        self.notify_refusals || self.notify_high_risk
    }

    /// Push the settings that live in `toolog-core` into it.
    ///
    /// Redaction of the evidence store is read on the write path, far from
    /// anything that knows what a preference is, so the preference has to be
    /// handed over rather than looked up.
    pub fn apply(&self) {
        toolog_core::redact::set_evidence_redaction(self.redact_evidence);
        toolog_ingest::discover::set_excluded(self.excluded_projects.clone());
    }
}

/// Where the file lives: beside the database, not in the bundle.
#[must_use]
pub fn path() -> Option<PathBuf> {
    toolog_core::db::default_path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("prefs.json")))
}

/// Read the preferences, or the all-off default.
#[must_use]
pub fn load() -> Prefs {
    let Some(path) = path() else {
        return Prefs::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Prefs::default();
    };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        tracing::warn!(error = %e, path = %path.display(), "preferences unreadable; using defaults");
        Prefs::default()
    })
}

/// Write the preferences. The only file this process owns outside the store.
pub fn save(prefs: &Prefs) -> anyhow::Result<()> {
    let path =
        path().ok_or_else(|| anyhow::anyhow!("no data directory to write preferences to"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(prefs)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_switch_is_off_until_someone_turns_it_on() {
        let prefs = Prefs::default();
        assert!(!prefs.notify_refusals);
        assert!(!prefs.notify_high_risk);
        assert!(!prefs.redact_evidence);
        assert!(prefs.excluded_projects.is_empty());
        assert!(!prefs.any());
    }

    #[test]
    fn a_partial_file_keeps_the_defaults_for_what_it_does_not_mention() {
        let prefs: Prefs =
            serde_json::from_str(r#"{"notify_refusals": true}"#).expect("partial prefs");
        assert!(prefs.notify_refusals);
        assert!(
            !prefs.notify_high_risk,
            "a switch the file does not mention stays off"
        );
    }
}
