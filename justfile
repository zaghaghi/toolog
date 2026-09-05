# toolog — task runner
# `just` with no arguments lists available recipes.

default:
    @just --list

# Install the frontend's dependencies. Needed once, and after package.json moves.
ui-install:
    npm --prefix ui ci

# Type-check and bundle the window into ui/dist (Phase 5.1).
ui:
    npm --prefix ui run build

# Type-check the frontend without bundling.
ui-check:
    npm --prefix ui run check

# The frontend's tests.
ui-test:
    npm --prefix ui test

# Build the workspace (debug). The window is embedded, so it is built first.
build: ui
    cargo build --workspace --all-targets

# Build optimized.
release: ui
    cargo build --workspace --release

# Run the test suite.
test:
    cargo test --workspace --all-targets

# Format all code.
fmt:
    cargo fmt --all

# Everything CI checks, in CI's order. Run before pushing.
lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings

# Format, lint and test in one pass — Rust and the frontend.
check: fmt lint ui-check ui-test test

# Run the application. The window is compiled in, so it is bundled first.
run *ARGS: ui
    cargo run --bin toolog -- {{ARGS}}

# Report the state of the Claude Code integration. Read-only; `--fix` mutates.
doctor *ARGS:
    cargo run --bin toolog -- doctor {{ARGS}}

# Import existing history from ~/.claude/projects.
backfill *ARGS:
    cargo run --bin toolog -- backfill {{ARGS}}

# Reconcile the two ingestion lanes (ADR-0009).
verify *ARGS:
    cargo run --bin toolog -- verify {{ARGS}}

# Regenerate ui/src/bindings.ts from the Rust command surface.
bindings:
    cargo test -p toolog-app bindings

# Install or remove the login agent that keeps capture running.
agent action="status":
    cargo run --bin toolog -- agent {{action}}

# Build the distributable .app / .dmg. Arrives in Phase 8.
bundle:
    @echo "Phase 8 — not implemented. See docs/phases/08-packaging-distribution.md"
    @exit 1

# Remove build artifacts.
clean:
    cargo clean
