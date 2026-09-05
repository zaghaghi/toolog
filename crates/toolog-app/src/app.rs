//! Process lifecycle: one resident process, on demand window ([ADR-0007]).
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, RunEvent};
use toolog_cli::capture::Capture;
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
            let capture = start_capture(&handle, db_path.clone())?;
            let endpoint_changed = capture.port_changed();
            let endpoint = capture.endpoint();

            app.manage(AppState::new(db_path.clone(), capture)?);
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
fn start_capture(handle: &AppHandle, db_path: std::path::PathBuf) -> anyhow::Result<Capture> {
    let emitter = handle.clone();
    let sink: toolog_cli::capture::LiveSink = Arc::new(move |call: &ToolCall| {
        if let Err(e) = emitter.emit("live_tool_call", call) {
            tracing::debug!(error = %e, "live event not delivered");
        }
    });

    let addr = toolog_otlp::port::default_addr();
    tauri::async_runtime::block_on(Capture::start(db_path, addr, None, Some(sink)))
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
