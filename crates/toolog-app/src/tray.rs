//! The menu-bar item.
//!
//! [ADR-0007] makes this a requirement rather than a convenience: a background
//! process silently recording every command an agent runs, with no visible
//! indicator, is the wrong posture for a tool asking to be trusted. The icon
//! dims when capture is paused, and the first menu line always says what is
//! being recorded and how much.
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

use std::sync::Arc;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

use crate::state::AppState;
use crate::window;

const TRAY_ID: &str = "toolog";

const ICON_ACTIVE: &[u8] = include_bytes!("../icons/tray.png");
const ICON_PAUSED: &[u8] = include_bytes!("../icons/tray-paused.png");

/// The menu items whose text and state change while the app runs.
pub(crate) struct Handles {
    status: MenuItem<Wry>,
    pause: CheckMenuItem<Wry>,
    login: CheckMenuItem<Wry>,
}

/// Build the tray item and register its menu.
pub(crate) fn install(app: &AppHandle) -> anyhow::Result<()> {
    let status = MenuItem::with_id(app, "status", "Starting…", false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open toolog", true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, "pause", "Pause Capture", true, false, None::<&str>)?;
    let backfill = MenuItem::with_id(app, "backfill", "Import History…", true, None::<&str>)?;
    let logs = MenuItem::with_id(app, "logs", "Reveal Logs", true, None::<&str>)?;
    let login = CheckMenuItem::with_id(
        app,
        "login",
        "Start at Login",
        toolog_cli::launchagent::is_supported(),
        toolog_cli::launchagent::status(&toolog_cli::settings::home_dir()).installed,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit toolog", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &open,
            &pause,
            &backfill,
            &PredefinedMenuItem::separator(app)?,
            &logs,
            &login,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    app.manage(Arc::new(Handles {
        status,
        pause,
        login,
    }));

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(ICON_ACTIVE)?)
        // A template image follows the menu bar's own light and dark themes.
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("toolog")
        .on_menu_event(on_menu_event)
        .build(app)?;

    refresh(app);
    Ok(())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the signature is Tauri's; the callback is handed an owned MenuEvent"
)]
fn on_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let app = app.clone();
    match event.id().as_ref() {
        "open" => {
            if let Err(e) = window::show(&app) {
                tracing::error!(error = %e, "could not open the window");
            }
        }
        "pause" => toggle_pause(&app),
        "backfill" => run_backfill(&app),
        "logs" => reveal_logs(&app),
        "login" => toggle_login_agent(&app),
        "quit" => {
            // Explicit: capture stops, and the login agent does not undo it
            // because the plist restarts only on a *failed* exit.
            if let Some(state) = app.try_state::<Arc<AppState>>() {
                state.shutdown();
            }
            app.exit(0);
        }
        other => tracing::debug!(id = other, "unhandled tray menu item"),
    }
}

fn toggle_pause(app: &AppHandle) {
    let Some(state) = app.try_state::<Arc<AppState>>() else {
        return;
    };
    let result = state.with_capture(|capture| {
        if capture.is_paused() {
            capture.resume();
        } else {
            capture.pause();
        }
        Ok(capture.is_paused())
    });
    match result {
        Ok(paused) => tracing::info!(paused, "capture toggled from the tray"),
        Err(e) => tracing::error!(error = %e, "could not toggle capture"),
    }
    refresh(app);
}

fn run_backfill(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(state) = app.try_state::<Arc<AppState>>() else {
            return;
        };
        let outcome = state.with_capture(|capture| {
            capture.catch_up();
            Ok(())
        });
        if let Err(e) = outcome {
            tracing::error!(error = %e, "backfill could not be started");
            return;
        }
        notify(
            &app,
            "Importing history",
            "Reading ~/.claude/projects in the background.",
        );
        refresh(&app);
    });
}

fn reveal_logs(app: &AppHandle) {
    let result = toolog_cli::logging::log_dir()
        .map_err(anyhow::Error::from)
        .and_then(|dir| {
            std::fs::create_dir_all(&dir)?;
            window::reveal(app, &dir)
        });
    if let Err(e) = result {
        tracing::error!(error = %e, "could not reveal the logs");
    }
}

fn toggle_login_agent(app: &AppHandle) {
    let home = toolog_cli::settings::home_dir();
    let installed = toolog_cli::launchagent::status(&home).installed;

    let result = if installed {
        toolog_cli::launchagent::uninstall(&home).map(|()| false)
    } else {
        std::env::current_exe()
            .map_err(|e| toolog_cli::launchagent::AgentError::Io {
                path: std::path::PathBuf::from("current exe"),
                source: e,
            })
            .and_then(|exe| {
                let log_dir = toolog_cli::logging::log_dir().unwrap_or_else(|_| home.clone());
                toolog_cli::launchagent::install(&home, &exe, &log_dir).map(|_| true)
            })
    };

    match result {
        Ok(now_installed) => notify(
            app,
            if now_installed {
                "Starting at login"
            } else {
                "Login agent removed"
            },
            if now_installed {
                "toolog will run in the background so nothing is missed."
            } else {
                "Capture only runs while toolog is open."
            },
        ),
        Err(e) => tracing::error!(error = %e, "could not change the login agent"),
    }
    refresh(app);
}

/// Bring the tray in line with the current state.
pub(crate) fn refresh(app: &AppHandle) {
    let Some(handles) = app.try_state::<Arc<Handles>>() else {
        return;
    };
    let Some(state) = app.try_state::<Arc<AppState>>() else {
        return;
    };

    let status = state.with_capture(|capture| Ok(capture.status()?));
    let (line, paused) = match &status {
        Ok(s) if s.paused => (format!("Paused · {} calls stored", s.tool_calls), true),
        Ok(s) => (
            format!(
                "Capturing on {} · {} events today",
                s.endpoint.trim_start_matches("http://"),
                s.events_today
            ),
            false,
        ),
        Err(e) => (format!("Not capturing — {e}"), true),
    };

    let _ = handles.status.set_text(&line);
    let _ = handles.pause.set_checked(paused);
    let _ = handles
        .login
        .set_checked(toolog_cli::launchagent::status(&toolog_cli::settings::home_dir()).installed);

    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let bytes = if paused { ICON_PAUSED } else { ICON_ACTIVE };
        if let Ok(icon) = Image::from_bytes(bytes) {
            let _ = tray.set_icon(Some(icon));
            let _ = tray.set_icon_as_template(true);
        }
        let _ = tray.set_tooltip(Some(&line));
    }
}

/// A desktop notification, best-effort.
pub(crate) fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::debug!(error = %e, "notification not shown");
    }
}
