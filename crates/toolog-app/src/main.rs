//! The single `toolog` executable ([ADR-0007]).
//!
//! One process owns the OTLP listener, the transcript tailer, the sole database
//! write handle, the menu-bar item and the window — and the same binary serves
//! the CLI by argv dispatch. One artifact to install, sign, notarize and update.
//!
//! Tauri, the tray and the LaunchAgent arrive in Phase 4.
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

fn main() {
    println!(
        "{} {} — scaffolding. See docs/README.md for the phase plan.",
        toolog_core::constants::APP_NAME,
        env!("CARGO_PKG_VERSION"),
    );
}
