# ADR-0007 — One resident process, LaunchAgent-managed

- **Status:** Accepted
- **Date:** 2026-09-04
- **Relates to:** [ADR-0001](0001-tauri-2-for-the-desktop-shell.md), [ADR-0003](0003-sqlite-as-the-embedded-store.md), [ADR-0005](0005-embedded-otlp-receiver.md)

## Context

Two requirements pull in different directions.

**Capture must be continuous.** The OTLP endpoint has to be listening whenever Claude Code runs, or
the decision and cost layer is lost for that session — permanently, since OTEL events are not
replayable from disk the way transcripts are.

**The GUI is occasional.** Nobody keeps an audit timeline open all day. It is opened when there is a
question to answer.

The obvious resolution is a headless daemon plus a separate GUI that attaches to it. That gives
clean separation — but it means two binaries, an IPC layer, two lifecycles to install and debug, and
a direct conflict with the brief's one-artifact install constraint (ADR-0001, ADR-0005).

There is also a database consideration. ADR-0003 chose SQLite; concurrent writers across processes
would mean lock contention and busy-timeout tuning. A single writer removes the problem entirely
rather than managing it.

## Decision

**One process owns everything: the OTLP listener, the transcript tailer, the sole database write
handle, the tray item, and the window.**

- Resident by default, presenting a **menu-bar item** — status (collector up/down, events today),
  Open Window, Pause Capture, Quit.
- **The window is created on demand.** Closing it hides the window; it does not exit the process.
- A **macOS LaunchAgent with `KeepAlive`** starts it at login and restarts it if it dies. Installed
  and removed from the UI, never silently.
- `tauri-plugin-single-instance` guarantees exactly one process, so a second launch focuses the
  existing window instead of racing for the database and the port.
- The same binary provides the CLI (`doctor`, `backfill`, `verify`, `export`) by argv dispatch.

## Consequences

**Positive**

- One artifact to install, sign, notarize and update — the constraint that drove ADR-0005 too.
- One database writer: no cross-process locking, no WAL contention, no busy-timeout tuning.
- The live view needs no IPC. Ingestion emits to the UI through an in-process channel.
- The tray gives capture an honest, visible presence. A background process silently recording tool
  calls with no indicator would be the wrong posture for a tool asking to be trusted (ADR-0008).

**Negative**

- A resident WebView app costs memory even when idle (~150 MB, per ADR-0001). Mitigated because the
  window is not created until first opened.
- If the user quits from the tray, capture stops. Accepted and made visible: quitting is explicit,
  the tray shows collector state, and `toolog verify` reports the resulting gap rather than hiding
  it. Transcripts are still read from disk afterwards, so only the OTEL layer is affected.
- Coupling the UI to the collector means a UI crash takes capture down with it. Mitigated by
  `KeepAlive` and by writing to `raw_event` before any projection work (ADR-0004).

**Neutral**

- The Linux equivalent is a systemd user unit, and Windows a registry Run key or scheduled task.
  Neither is in v1, but nothing here forecloses them.

## Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| Headless daemon + separate GUI | Two artifacts, an IPC layer and two lifecycles, contradicting the one-artifact install. Cleanest on paper, worst for the stated constraints. |
| GUI-only, no resident process | Capture would only happen while the window is open — the app would miss most of what it exists to record. |
| Launch on demand from a hook or wrapper | Requires code on Claude Code's path (rejected in ADR-0002) and would drop the first events of every session during startup. |
| `launchd` `KeepAlive` on a headless mode, GUI spawned separately from the same binary | Still one artifact, but reintroduces two processes and cross-process SQLite writes for no benefit over the single-process model. |
