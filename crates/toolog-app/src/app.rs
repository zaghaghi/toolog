//! Process lifecycle: one resident process, on demand window ([ADR-0007]).
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

use std::sync::{Arc, RwLock};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, RunEvent};
use toolog_cli::capture::Capture;
use toolog_cli::prefs::Prefs;
use toolog_core::model::ToolCall;

use crate::state::AppState;
use crate::{commands, tray, window};

/// How often the tray's status line is refreshed.
const TRAY_REFRESH: Duration = Duration::from_secs(5);

/// Run the desktop application.
///
/// `background` comes from the login agent: come up resident, with no window.
pub(crate) fn run(background: bool) -> anyhow::Result<()> {
    let guard = toolog_cli::logging::init_app();
    tracing::info!(background, "toolog starting");

    let db_path = toolog_core::db::default_path()?;

    tauri::Builder::default()
        // A second launch focuses the window instead of racing for the port and
        // the database, which is what makes the single-writer model safe.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Err(e) = window::show(app) {
                tracing::error!(error = %e, "could not focus the existing window");
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(commands::handler())
        .setup(move |app| {
            // A menu-bar app, not a Dock app: no icon, no app switcher entry.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();
            // Read once at startup and shared with the live sink, so deciding
            // whether to notify costs no disk access on the arrival path.
            let saved = toolog_cli::prefs::load();
            // Evidence redaction is read on the write path, so it has to be in
            // force before capture starts, not when the window first asks.
            saved.apply();
            let prefs = Arc::new(RwLock::new(saved));
            let capture = start_capture(&handle, db_path.clone(), Arc::clone(&prefs))?;
            let endpoint_changed = capture.port_changed();
            let endpoint = capture.endpoint();

            app.manage(AppState::new(db_path.clone(), capture, prefs)?);
            tray::install(&handle)?;
            spawn_tray_refresh(&handle);

            if endpoint_changed {
                on_port_conflict(&handle, &endpoint);
            }

            // The window opens on demand — except on a machine where nothing is
            // configured yet, where the whole point is to say so.
            if !background || needs_setup() {
                window::show(&handle)?;
            }

            Ok(())
        })
        .build(tauri::generate_context!())?
        .run(move |app, event| {
            // The rotating log writer stops when this guard drops, so it has to
            // outlive the event loop rather than the setup closure.
            let _keep_logging = &guard;

            match event {
                // Closing the last window must not end the process; capture
                // continues, and Quit from the tray is the way out.
                RunEvent::ExitRequested {
                    code: None, api, ..
                } => {
                    api.prevent_exit();
                }
                RunEvent::Exit => {
                    if let Some(state) = app.try_state::<Arc<AppState>>() {
                        state.shutdown();
                    }
                    tracing::info!("toolog stopped");
                }
                _ => {}
            }
        });

    Ok(())
}

/// Start both ingestion lanes, wired to the UI's live event.
fn start_capture(
    handle: &AppHandle,
    db_path: std::path::PathBuf,
    prefs: Arc<RwLock<Prefs>>,
) -> anyhow::Result<Capture> {
    let emitter = handle.clone();
    let notifier = handle.clone();
    let watch_db = db_path.clone();
    let sink: toolog_cli::capture::LiveSink = Arc::new(move |call: &ToolCall| {
        if let Err(e) = emitter.emit("live_tool_call", call) {
            tracing::debug!(error = %e, "live event not delivered");
        }
        let switches = prefs.read().map(|p| p.clone()).unwrap_or_default();
        if switches.any() {
            notify_about(&notifier, &watch_db, call, &switches);
        }
    });

    let addr = toolog_otlp::port::default_addr();
    tauri::async_runtime::block_on(Capture::start(db_path, addr, None, Some(sink)))
}

/// A native notification about one call, if the user asked for it (task 6.12).
///
/// Runs on the live thread, so it does the cheap test first: a refusal is a
/// column, and only a call that is not already a refusal is put to the rules.
fn notify_about(handle: &AppHandle, db_path: &std::path::Path, call: &ToolCall, prefs: &Prefs) {
    use tauri_plugin_notification::NotificationExt as _;

    // The rules are only consulted when that switch is on *and* the cheaper
    // test did not already produce a notification.
    let rule = || {
        (prefs.notify_high_risk && !(prefs.notify_refusals && call.is_rejected()))
            .then(|| high_risk_rule(db_path, &call.tool_use_id))
            .flatten()
    };
    let Some((title, body)) = notification_for(call, prefs, rule) else {
        return;
    };

    if let Err(e) = handle
        .notification()
        .builder()
        .title(title)
        .body(truncate(&body, 180))
        .show()
    {
        tracing::debug!(error = %e, "notification not shown");
    }
}

/// What to say about a call, or nothing at all.
///
/// Separated from showing it so the policy is testable without a window. The
/// policy is small and worth stating: **nothing fires unless a switch is on**,
/// a refusal wins over a rule hit because it is the more specific fact, and
/// `rule` is a closure so the rules are not consulted when no switch would use
/// the answer.
fn notification_for(
    call: &ToolCall,
    prefs: &Prefs,
    rule: impl FnOnce() -> Option<String>,
) -> Option<(String, String)> {
    if !prefs.any() {
        return None;
    }
    let what = call
        .input_summary
        .as_deref()
        .or(call.target_path.as_deref())
        .unwrap_or("")
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    let tool = call.tool_name.as_deref().unwrap_or("a tool");

    if prefs.notify_refusals && call.is_rejected() {
        return Some((format!("{tool} was refused"), what));
    }
    if prefs.notify_high_risk {
        return rule().map(|title| (title, format!("{tool}: {what}")));
    }
    None
}

/// The title of the first high-severity rule this call trips, if any.
///
/// Opens its own connection: this runs on the live thread, which has no read
/// handle of its own, and the alternative is contending with the window's.
fn high_risk_rule(db_path: &std::path::Path, tool_use_id: &str) -> Option<String> {
    use toolog_core::rules::Severity;

    let rules = toolog_cli::commands::rules().ok()?;
    let db = toolog_core::Db::open(db_path).ok()?;
    rules
        .iter()
        .filter(|r| r.severity == Severity::High)
        .find(|r| toolog_core::rules::matches(db.conn(), r, tool_use_id).unwrap_or(false))
        .map(|r| r.title.clone())
}

/// Cut a notification body to something a banner will actually show.
fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let kept: String = text.chars().take(limit - 1).collect();
    format!("{kept}…")
}

/// Keep the menu-bar line honest without the user having to open it.
fn spawn_tray_refresh(handle: &AppHandle) {
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(TRAY_REFRESH);
        loop {
            ticker.tick().await;
            tray::refresh(&handle);
        }
    });
}

/// The receiver had to move: Claude Code's configuration now points nowhere.
///
/// Left alone this is the worst failure this tool can have — everything looks
/// fine and nothing is recorded. So the endpoint is rewritten to match, but
/// only when the configured value is already one of ours; a foreign endpoint is
/// reported and left untouched (ADR-0006).
fn on_port_conflict(handle: &AppHandle, endpoint: &str) {
    let Ok(paths) = toolog_cli::doctor::Paths::detect() else {
        return;
    };
    let stack = toolog_cli::settings::Stack::read(&paths.cwd, &paths.home);
    let configured = stack.effective(toolog_cli::settings::LOGS_ENDPOINT_KEY);

    let ours = configured.is_some_and(|(_, value)| toolog_cli::settings::is_loopback_url(value));

    if ours {
        match toolog_cli::settings::apply_fix(&stack, endpoint) {
            Ok(_) => {
                tracing::warn!(endpoint, "preferred port was taken; endpoint rewritten");
                tray::notify(
                    handle,
                    "toolog moved to another port",
                    &format!("Claude Code now exports to {endpoint}. Restart any open session."),
                );
            }
            Err(e) => tracing::error!(error = %e, "could not rewrite the endpoint"),
        }
    } else {
        tray::notify(
            handle,
            "toolog is listening elsewhere",
            &format!("Port conflict: capture is on {endpoint}. Run `toolog doctor` to reconnect."),
        );
    }
}

/// Whether the first-run wizard has anything to say.
fn needs_setup() -> bool {
    toolog_cli::doctor::Paths::detect().map_or(true, |paths| {
        !toolog_cli::doctor::report(&paths).configured()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call() -> ToolCall {
        ToolCall {
            tool_use_id: "toolu_1".into(),
            tool_name: Some("Bash".into()),
            input_summary: Some("cat .env\nsecond line".into()),
            ..ToolCall::default()
        }
    }

    fn refused() -> ToolCall {
        ToolCall {
            decision: Some("reject".into()),
            ..call()
        }
    }

    /// Task 6.12's first requirement, as an assertion: off by default.
    #[test]
    fn nothing_is_notified_until_a_switch_is_on() {
        let prefs = Prefs::default();
        assert!(notification_for(&call(), &prefs, || Some("a rule".into())).is_none());
        assert!(notification_for(&refused(), &prefs, || None).is_none());
    }

    #[test]
    fn each_switch_notifies_only_about_its_own_kind() {
        let refusals = Prefs {
            notify_refusals: true,
            ..Prefs::default()
        };
        let (title, body) = notification_for(&refused(), &refusals, || None).expect("a refusal");
        assert_eq!(title, "Bash was refused");
        assert_eq!(body, "cat .env", "the command's first line, not its body");
        assert!(
            notification_for(&call(), &refusals, || Some("a rule".into())).is_none(),
            "a rule hit is not this switch's business"
        );

        let risk = Prefs {
            notify_high_risk: true,
            ..Prefs::default()
        };
        let (title, body) =
            notification_for(&call(), &risk, || Some("Credentials read".into())).expect("a hit");
        assert_eq!(title, "Credentials read");
        assert_eq!(body, "Bash: cat .env");
        assert!(
            notification_for(&call(), &risk, || None).is_none(),
            "a call that trips nothing says nothing"
        );
    }

    #[test]
    fn a_refusal_wins_over_a_rule_hit_because_it_is_the_more_specific_fact() {
        let both = Prefs {
            notify_refusals: true,
            notify_high_risk: true,
            ..Prefs::default()
        };
        let (title, _) =
            notification_for(&refused(), &both, || Some("Credentials read".into())).expect("one");
        assert_eq!(title, "Bash was refused");
    }

    #[test]
    fn a_body_is_cut_to_something_a_banner_will_show() {
        let long = "x".repeat(400);
        let cut = truncate(&long, 180);
        assert_eq!(cut.chars().count(), 180);
        assert!(cut.ends_with('…'));
        assert_eq!(
            truncate("short", 180),
            "short",
            "and short text is left alone"
        );
    }
}
