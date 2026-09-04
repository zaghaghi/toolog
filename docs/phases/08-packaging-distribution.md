# Phase 8 — Packaging & distribution → **v1.0**

**Goal:** the brief's second constraint, delivered — dead simple installation, one artifact, by
download or `brew`.

**Depends on:** all previous phases.
**Governed by:** [ADR-0001](../adr/0001-tauri-2-for-the-desktop-shell.md),
[ADR-0008](../adr/0008-local-only-zero-egress.md).

## Tasks

- [ ] **8.1** Universal macOS binary (`aarch64-apple-darwin` + `x86_64-apple-darwin`), `.app`
  bundle, `.dmg`. Reproducible from `just bundle`.
- [ ] **8.2** Codesign with a Developer ID certificate, notarize, and staple.
  **Verify on a genuinely clean machine** — a first launch with a Gatekeeper warning fails the
  "dead simple" requirement regardless of what the docs say.
- [ ] **8.3** Homebrew cask in a tap; `brew install --cask <name>` tested end-to-end on a clean
  machine, including `brew uninstall`.
- [ ] **8.4** GitHub Releases with checksums and release notes; CI builds and signs on tag.
- [ ] **8.5** `tauri-plugin-updater` with signed manifests. Per ADR-0008 this is the **only**
  permitted network call: **off by default, opt-in at first run, sends no user data, and named
  explicitly in both the README and `PRIVACY.md`.** An undisclosed exception would be worse than
  having no updater.
- [ ] **8.6** **Uninstall path** — as carefully built as the install:
  - Remove the LaunchAgent
  - **Revert the `settings.json` `env` block from the Phase 4 backup**, restoring the file rather
    than deleting keys blindly
  - Offer to delete the database, defaulting to keeping it
  - Documented as a single command as well as a UI action
- [ ] **8.7** Linux `.AppImage` and `.deb` — stretch goal. Cheap only if no path handling assumed
  macOS from Phase 1 onward (see 1.1); verify that held.
- [ ] **8.8** `README` with real screenshots of all four views, the two-lane architecture diagram,
  the privacy posture up front, and honest limitations (cost data only for live-captured sessions;
  capture stops when the app is quit).
- [ ] **8.9** First-run experience measured end to end: **install → consent → backfill → first
  useful answer.** Target under two minutes. If it is not, the install is not "dead simple" and
  something above needs fixing.

## Exit criteria

- `brew install --cask <name>` on a clean machine launches with no Gatekeeper warning.
- The first-run wizard configures telemetry, backfills history and lands on a populated timeline
  without the user opening a terminal.
- Uninstall leaves `~/.claude/settings.json` byte-identical to its pre-install state.
- `PRIVACY.md` and the README accurately describe the shipped behaviour, including the updater.
