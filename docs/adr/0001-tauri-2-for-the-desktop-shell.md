# ADR-0001 — Tauri 2 for the desktop shell

- **Status:** Accepted
- **Date:** 2026-09-04
- **Deciders:** Project owner
- **Relates to:** [ADR-0003](0003-sqlite-as-the-embedded-store.md), [ADR-0007](0007-single-resident-process.md)

## Context

The brief asks for a "simple but beautiful GUI application that lists tool calls", distributed as
"one binary file to install either by download or installation tools like brew", built in Rust,
with Tauri or GPUI suggested.

The application is, structurally, four views over one table: a virtualized timeline, a findings
list, a small set of charts, and a live tail. It needs a menu-bar presence and must stay resident
so the OTLP listener is up whenever Claude Code runs (see ADR-0007).

> **Phase 9: three views, not four.** The charts and the live tail were removed after use — see
> [ADR-0010](0010-no-cost-reporting.md) and [Phase 9](../phases/09-subtraction.md). This changes
> nothing here except to make the argument shorter: three views over a table justify a heavy
> component framework even less than four did.

Three candidates were weighed.

**Tauri 2** — mature and stable, with official plugins (`autostart`, `single-instance`,
`notification`, `opener`) and core tray support that map directly onto the resident-process model.
A web frontend is the fastest route to a genuinely polished UI, and virtualized tables, diff
rendering and charts are solved problems there. Bundling, code signing and notarization are
first-class.

**GPUI** — Zed's GPU-accelerated framework. Produces a true single static binary, is pure Rust and
looks excellent. But it has no stable crates.io release, so consuming it means a git dependency on
the Zed repository with an API that churns between commits. Documentation is thin, Windows is
unsupported, and menu-bar/tray support is not first-class. Every widget the app needs — virtual
scroll, sortable table, diff view, charts — would be hand-built.

**egui/eframe** — a genuine single static binary, stable on crates.io, cross-platform, and quick to
build tables and filters in. But immediate mode carries a recognizable look that takes deliberate
work to escape, and "beautiful" is an explicit requirement rather than a nice-to-have.

## Decision

**Use Tauri 2.**

The frontend is TypeScript with Vite and no heavy component framework — the UI is a few views over
a table, and keeping the dependency surface small keeps the bundle and the `.app` honest.

## Consequences

**Positive**

- Tray, autostart, single-instance and notifications are supported paths, not experiments.
- The richest part of the UI (virtualized timeline, diff rendering, charts) uses mature libraries.
- Signing, notarization and `brew install --cask` are well-trodden with Tauri's bundler.
- Cross-platform later (Linux, and Windows if wanted) stays open.

**Negative — accepted**

- A WebView costs roughly 150 MB RSS. Acceptable for a resident audit tool on a developer machine.
- The artifact is a `.app` bundle, not literally one file. A signed `.dmg` and a Homebrew cask meet
  the intent of the brief's second constraint; ADR-0008's privacy posture is unaffected. This is the
  one place the plan knowingly reads the brief's spirit over its letter.
- A Node toolchain is required at build time, though not at runtime.

**Neutral**

- The frontend never touches the database; it goes through typed Tauri commands (see ADR-0003).

## Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| GPUI | No stable release, git dependency on Zed, API churn, thin docs, no Windows, weak tray support, and every widget hand-built. Realistically 3–5× the UI effort for this app's shape. |
| egui/eframe | Real single-binary win, but reaching "beautiful" fights the immediate-mode idiom, and diff/chart/virtual-table work is all bespoke. |
| Web app + local server | Would violate the single-artifact install and put the UI a browser tab away from a tool meant to be glanceable. |
