# Phase 4 — App shell & lifecycle

**Goal:** it installs itself, stays resident, and the frontend can query. No views yet — this is the
plumbing every view sits on.

**Depends on:** Phases 1–3. **Unblocks:** Phases 5, 6.
**Governed by:** [ADR-0001](../adr/0001-tauri-2-for-the-desktop-shell.md),
[ADR-0006](../adr/0006-configure-via-settings-env-block.md),
[ADR-0007](../adr/0007-single-resident-process.md).

## Tasks

- [ ] **4.1** Tauri 2 application with `tauri-plugin-single-instance`, `tauri-plugin-autostart`,
  `tauri-plugin-notification`, `tauri-plugin-opener`. Tray via core `TrayIconBuilder`.
- [ ] **4.2** Menu-bar item: collector status (up/down, events today), Open Window, Pause Capture,
  Run Backfill, Preferences, Quit. Icon reflects capture state — a silent recorder with no visible
  indicator would be the wrong posture (ADR-0007).
- [ ] **4.3** Window created **on demand**; closing hides it rather than exiting the process.
- [ ] **4.4** Start the OTLP receiver and the transcript tailer as background tasks owned by the app
  process. One database write handle, held by the core (ADR-0003).
- [ ] **4.5** macOS LaunchAgent plist with `KeepAlive`, installed and removed from the UI —
  never silently. Store it at `~/Library/LaunchAgents/`.
- [ ] **4.6** **`toolog doctor`** — the install experience, and most of the future support burden.
  Read-only by default, reporting:
  - Is `CLAUDE_CODE_ENABLE_TELEMETRY` set, and where in the settings precedence stack?
  - Is the logs endpoint pointing at our port?
  - Is the receiver reachable (`/healthz`)?
  - Is `~/.claude/projects` present and readable? How many transcripts, how many already ingested?
  - Is a **managed/enterprise settings file** overriding the user file? (It takes precedence — say
    so plainly instead of appearing broken.)
- [ ] **4.7** `doctor --fix` writes the `env` block per ADR-0006. All four rules are hard
  requirements: **merge never overwrite**, **atomic write plus timestamped backup**, **per-signal
  variables only — never global `OTEL_EXPORTER_OTLP_ENDPOINT`**, and **abort with a clear message
  if a non-loopback OTEL logs endpoint is already configured**.
- [ ] **4.8** First-run wizard: state plainly what is captured and what never leaves the machine,
  run `doctor --fix` on explicit consent, offer backfill, offer autostart. Consent before the first
  write to a file the app does not own.
- [ ] **4.9** Tauri command surface, typed on both sides (generate TypeScript types from Rust —
  `ts-rs` or `specta` — so the boundary cannot drift):
  `query_timeline`, `get_tool_call`, `list_sessions`, `stats`, `search`, `collector_status`,
  `run_backfill`, `export`, plus a `live_tool_call` event stream.
- [ ] **4.10** CLI dispatch from the same binary: `doctor`, `backfill`, `verify`, `export`
  (ADR-0007 — one artifact).
- [ ] **4.11** Structured logging to a rotating local file, with a "Reveal logs" tray action.

## Exit criteria

- Fresh launch on a machine with no telemetry configured: `doctor` reports it off, `--fix` writes
  the block and leaves a backup, and the original file's other keys are untouched.
- Restoring the backup returns `settings.json` byte-identical to its pre-`fix` state.
- With the app running, a real Claude Code session in another terminal produces rows carrying
  `duration_ms` and `decision_source`.
- Quitting from the tray stops capture; relaunching resumes it with no duplicates.
- A second launch focuses the existing window instead of starting a second process.
