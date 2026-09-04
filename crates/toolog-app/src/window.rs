//! The window, created on demand ([ADR-0007]).
//!
//! Nobody keeps an audit timeline open all day; it is opened when there is a
//! question to answer. So the process starts with no window at all, builds one
//! the first time it is asked for, and **hides rather than closes** it — the
//! WebView is expensive to create, and closing it must not stop capture.
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// The main window's label.
pub(crate) const MAIN: &str = "main";

/// Show the window, creating it if this is the first time.
pub(crate) fn show(app: &AppHandle) -> anyhow::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN) {
        window.show()?;
        window.unminimize()?;
        window.set_focus()?;
        return Ok(());
    }

    let window = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("toolog")
        .inner_size(1120.0, 760.0)
        .min_inner_size(720.0, 480.0)
        .build()?;

    // Closing hides. Quitting is a deliberate act from the tray, so that the
    // difference between "I am done looking" and "stop recording" stays real.
    let handle = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = handle.hide();
        }
    });

    window.set_focus()?;
    Ok(())
}

/// Reveal a directory in the file manager.
pub(crate) fn reveal(app: &AppHandle, path: &std::path::Path) -> anyhow::Result<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|e| anyhow::anyhow!("could not reveal {}: {e}", path.display()))
}
