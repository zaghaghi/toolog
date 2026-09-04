# toolog — task runner
# `just` with no arguments lists available recipes.

default:
    @just --list

# Build the workspace (debug).
build:
    cargo build --workspace --all-targets

# Build optimized.
release:
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

# Format, lint and test in one pass.
check: fmt lint test

# Run the application.
run *ARGS:
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
