# Phase 0 — Foundation & decisions

**Goal:** the repository, workspace and decision record exist on disk. No product behaviour yet.

**Depends on:** nothing. **Unblocks:** every later phase.

## Tasks

- [x] **0.1** Cargo workspace at the repo root with five member crates:
  `toolog-core`, `toolog-ingest`, `toolog-otlp`, `toolog-cli`, `toolog-app`.
  Pin `edition` and a documented MSRV (local toolchain is 1.95.0).
- [x] **0.2** Shared workspace dependencies and lints in the root `Cargo.toml`
  (`[workspace.dependencies]`, `[workspace.lints]`). `clippy::all` and
  `clippy::pedantic` selectively, denied in CI.
- [x] **0.3** Write `docs/adr/0001`–`0009`. **Done** — written ahead of the workspace so the
  decisions are settled before any code depends on them.
- [x] **0.4** Write `docs/phases/00`–`08`. **Done.**
- [x] **0.5** `justfile` with `build`, `test`, `lint`, `fmt`, `run`, `bundle`, `doctor`.
- [x] **0.6** `.github/workflows/ci.yml` — `cargo fmt --check`, `cargo clippy -D warnings`,
  `cargo test`, on macOS and Linux runners. Cache the SQLite build (ADR-0003 uses `bundled`,
  which is slow from cold).
- [x] **0.7** Decide the final product name. **Settled: `toolog`** — verified free on
  crates.io (`toolog`, `toolog-core`, `toolog-cli`) and as a Homebrew cask. It lives in
  exactly one constant, `toolog_core::constants::APP_NAME`, plus the package names.
- [x] **0.8** `README.md` — what it is, the two-lane architecture in three sentences, the privacy
  posture, install instructions stubbed until Phase 8.
- [x] **0.9** `PRIVACY.md` skeleton — filled in as Phase 7 lands. Stating the posture before
  building against it keeps ADR-0008 honest.
- [x] **0.10** `.gitignore` (Rust, Node, macOS, `*.db`, `fixtures/raw/`), `LICENSE`, and
  `docs/adr/README.md` indexing the ADRs.
- [x] **0.11** `git init` and an initial commit. The repo is not currently a git repository.

## Exit criteria

- `just build` and `just test` succeed on an empty workspace.
- CI is green on a pull request.
- Every ADR is readable standalone and states what it rejected.

## Outcome

Workspace builds clean on Rust 1.95, with `cargo fmt --check` and
`cargo clippy -D warnings` passing.

**Crate layout.** `toolog-cli` is a *library* of command implementations, not a binary.
[ADR-0007](../adr/0007-single-resident-process.md) ships one artifact, so `toolog-app`
provides the single `toolog` executable and dispatches CLI subcommands by argv. The same
binary hosts the menu-bar app from Phase 4.

**Lints.** `clippy::pedantic` is on, with three allows recorded in the root `Cargo.toml`.
`doc_markdown` was added during this phase: it fires on any CamelCase word, and these doc
comments are architectural prose full of proper nouns (LaunchAgent, OpenTelemetry,
SQLCipher). Backticking every one reads worse than the lint is worth.

**Dependencies** are declared in `[workspace.dependencies]` with versions pinned from
crates.io, but no member opts into them yet — a declaration there does not pull anything
into a build. Phase 1 adds the first real dependency.

**Open for the owner:**

- `repository` is absent from `[workspace.package]` — there is no git remote yet. Phase 8
  needs it for the Homebrew cask and release manifests.
- Licence is MIT, baked into every crate's metadata. Cheapest to change now.
