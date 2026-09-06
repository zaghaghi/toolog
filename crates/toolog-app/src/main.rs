//! The single `toolog` executable ([ADR-0007]).
//!
//! One process owns the OTLP listener, the transcript tailer, the sole database
//! write handle, the menu-bar item and the window — and the same binary serves
//! the CLI by argv dispatch. One artifact to install, sign, notarize and update.
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

mod app;
#[cfg(test)]
mod bindings;
mod commands;
mod llm;
mod state;
mod tray;
mod window;

use clap::Parser;
use toolog_cli::cli::Cli;

fn main() {
    let cli = Cli::parse();

    // A subcommand runs and exits. Only a bare invocation opens the
    // application, so `toolog export | jq` never leaves a window, a listener or
    // a resident process behind.
    if cli.command.is_some() {
        match toolog_cli::cli::run(&cli) {
            Ok(code) => std::process::exit(code),
            Err(e) => {
                eprintln!("toolog: {e:#}");
                std::process::exit(1);
            }
        }
    }

    if let Err(e) = app::run(cli.background) {
        eprintln!("toolog: {e:#}");
        std::process::exit(1);
    }
}
