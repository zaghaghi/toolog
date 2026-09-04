# Phase 4 — App shell & lifecycle

**Goal:** it installs itself, stays resident, and the frontend can query. No views yet — this is the
plumbing every view sits on.

**Depends on:** Phases 1–3. **Unblocks:** Phases 5, 6.
**Governed by:** [ADR-0001](../adr/0001-tauri-2-for-the-desktop-shell.md),
[ADR-0006](../adr/0006-configure-via-settings-env-block.md),
[ADR-0007](../adr/0007-single-resident-process.md).

## Tasks

- [x] **4.1** Tauri 2 application with `tauri-plugin-single-instance`, `tauri-plugin-autostart`,
  `tauri-plugin-notification`, `tauri-plugin-opener`. Tray via core `TrayIconBuilder`.
- [x] **4.2** Menu-bar item: collector status (up/down, events today), Open Window, Pause Capture,
  Run Backfill, Preferences, Quit. Icon reflects capture state — a silent recorder with no visible
  indicator would be the wrong posture (ADR-0007).
- [x] **4.3** Window created **on demand**; closing hides it rather than exiting the process.
- [x] **4.4** Start the OTLP receiver and the transcript tailer as background tasks owned by the app
  process. One database write handle, held by the core (ADR-0003).
- [x] **4.5** macOS LaunchAgent plist with `KeepAlive`, installed and removed from the UI —
  never silently. Store it at `~/Library/LaunchAgents/`.
- [x] **4.6** **`toolog doctor`** — the install experience, and most of the future support burden.
  Read-only by default, reporting:
  - Is `CLAUDE_CODE_ENABLE_TELEMETRY` set, and where in the settings precedence stack?
  - Is the logs endpoint pointing at our port?
  - Is the receiver reachable (`/healthz`)?
  - Is `~/.claude/projects` present and readable? How many transcripts, how many already ingested?
  - Is a **managed/enterprise settings file** overriding the user file? (It takes precedence — say
    so plainly instead of appearing broken.)
- [x] **4.7** `doctor --fix` writes the `env` block per ADR-0006. All four rules are hard
  requirements: **merge never overwrite**, **atomic write plus timestamped backup**, **per-signal
  variables only — never global `OTEL_EXPORTER_OTLP_ENDPOINT`**, and **abort with a clear message
  if a non-loopback OTEL logs endpoint is already configured**.
- [x] **4.8** First-run wizard: state plainly what is captured and what never leaves the machine,
  run `doctor --fix` on explicit consent, offer backfill, offer autostart. Consent before the first
  write to a file the app does not own.
- [x] **4.9** Tauri command surface, typed on both sides (generate TypeScript types from Rust —
  `ts-rs` or `specta` — so the boundary cannot drift):
  `query_timeline`, `get_tool_call`, `list_sessions`, `stats`, `search`, `collector_status`,
  `run_backfill`, `export`, plus a `live_tool_call` event stream.
- [x] **4.10** CLI dispatch from the same binary: `doctor`, `backfill`, `verify`, `export`
  (ADR-0007 — one artifact).
- [x] **4.11** Structured logging to a rotating local file, with a "Reveal logs" tray action.

## Exit criteria

- [x] Fresh launch on a machine with no telemetry configured: `doctor` reports it off, `--fix` writes
  the block and leaves a backup, and the original file's other keys are untouched.
- [x] Restoring the backup returns `settings.json` byte-identical to its pre-`fix` state.
- [x] With the app running, a real Claude Code session in another terminal produces rows carrying
  `duration_ms` and `decision_source`.
- [x] Quitting from the tray stops capture; relaunching resumes it with no duplicates.
- [x] A second launch focuses the existing window instead of starting a second process.

## Outcome

177 tests passing, `just lint` clean, CI extended with Tauri's Linux dependencies and a
check that the generated TypeScript bindings are committed.

### Verified on this machine, not only in tests

| Criterion | How it was checked |
|---|---|
| `doctor` on an unconfigured machine | Reported all six variables unset, receiver down, 43 transcripts / 43.6 MiB found, 0 ingested |
| `--fix` merges | Wrote the six keys into a real `~/.claude/settings.json` holding nine other keys; all nine survived with their order intact and `env` appended |
| Backup is a real revert | `md5` of the backup equals the pre-`fix` file exactly; a unit test performs the round trip |
| Live session end to end | `claude -p` in another directory produced `toolu_011nTh…` with `duration_ms = 103`, `decision_source = config`, `provenance = 3` — **the first row in the project's history witnessed by both lanes** |
| Relaunch without duplicates | Killed and restarted, then re-read all 47 transcripts: 12,053 lines, 2,171 already held, **zero duplicate content hashes** |
| Single instance | A second launch exited 0 immediately; one process remained |

Two halves were **not** driven end to end and are recorded as such: the tray's *Quit* item was not
clicked (there is no way to drive the macOS menu bar from this environment — the code path behind it
is the same `shutdown()` the capture tests exercise), and the window-focus half of the
single-instance check could only be observed as "the second process exited", not as a window coming
forward.

### The Phase 3 carry-over, and what it overturned

Phase 3 left one exit criterion open: *verify live that a denied tool call produces `decision=reject`
with no transcript body*. Done, and **the second half of that sentence is false.**

With `--permission-mode dontAsk`, a refused `Bash` call and a refused `Read` call both arrived as
`decision=reject`, `decision_source=config`, with the attempted command and target preserved. But
they carry `provenance = 3`: the transcript kept the `tool_use` block **and** a `tool_result` whose
content is the refusal message.

ADR-0002 and ADR-0009 both asserted that denied calls leave no transcript trace at all, and
ADR-0009 turned that into an inference — *OTEL-only ⇒ rejected*. **That inference would have
found zero of the two real rejections.** Both ADRs now carry a correction, `Reconciliation` gained a
`rejected` count read from the `decision` column, and a regression test asserts a refusal is
counted even when both lanes saw it.

The dual-lane design is unaffected, and the argument for it is now sharper than the one that was
assumed. The transcript says *that* something was refused, in English, inside a result string. Only
OTEL says **who refused it and under which rule** — `decision` and `decision_source` are columns you
can query, not prose you have to grep.

One case remains unmeasured: an **interactive** refusal, where a person presses no at the prompt.
That may well abort before any `tool_result` is written. Driving the interactive TUI was out of
reach; until it is measured, no code infers a rejection from provenance.

### A destructive bug the real data exposed

Exercising `toolog export --rejected` against the live store returned nothing, minutes after
`toolog verify` had counted two refusals. **`toolog backfill` had deleted them.**

`Backfill::run` finished by re-projecting the whole database, and re-projection clears every
projection table before replaying. It replayed with the transcript projector alone, so every
`decision`, `decision_source`, `duration_ms` and `api_request` row was dropped on the floor — on
every import, silently, with a cheerful summary printed.

Three changes:

- `Backfill::run` no longer re-projects. It projects incrementally through one projector shared
  across every file, which is what cross-file subagent attribution needed in the first place, and
  it clears nothing.
- `project::Chain` forwards each record to several projectors, and `TranscriptProjector` gained the
  lane guard the OTLP one already had.
- A genuine rebuild is `commands::reproject_all` — both lanes — reachable as
  `toolog backfill --reproject`. Two regression tests pin it: an import must not clear a lane it
  cannot rebuild, and a full re-projection must reproduce every lane.

**The live store was repaired by re-running the projection**, and lost nothing: `both = 3`,
`refused = 2`, exactly the figures from before the damage. That is [ADR-0004] working as designed —
the projections were wrong, the evidence never was. It is also the strongest argument the project
has yet produced for storing raw records verbatim, and it arrived by accident.

[ADR-0004]: ../adr/0004-store-raw-project-normalized.md

### Two flaky tests, and the bug behind one of them

The suite failed intermittently under load, roughly one run in three, and chasing it found a real
gap rather than a timing artifact.

**The tailer could leave a file's tail unread.** A burst is ingested while its last record is still
being flushed; the fragment is correctly not stored, and the offset stops before it — but if nothing
writes to that file again, no event ever wakes the watcher to collect it. A session that ends
immediately after its final tool call is exactly that shape. `Tail` now also sweeps every 30 seconds,
re-reading each transcript from its stored offset, which costs an open and a seek per file and makes
the watch self-healing instead of dependent on the next event arriving. A test writes one record and
then deliberately produces nothing further for the watcher to act on.

**The other was the test's own race.** `nothing_listening_is_down_not_an_error` bound an ephemeral
port, released it, and probed it — while the workspace runs its test binaries in parallel and one of
them will eventually be handed that number in the gap. It now probes port 1, which needs root to
bind and is in no ephemeral range. `a_free_port_is_used_as_is` had the same shape and retries
instead.

### Deviations from the plan

**`tauri-plugin-autostart` is not used.** It writes its own LaunchAgent plist at
`~/Library/LaunchAgents/<bundle id>.plist` — the same path ours uses — and its plist has no
`KeepAlive`, which ADR-0007 requires. Two components writing one file is a bug, not a feature, so
the `launchagent` module is the single owner. It is also the reason `KeepAlive` is a dictionary
(`SuccessfulExit: false`) rather than `true`: ADR-0007 asks both for a crash to be restarted *and*
for a deliberate Quit to stick, and only the dictionary form gives both.

**`timeline_count` was added to the command surface.** Not in the task list, and needed by every
view in it: a virtualized list cannot size its scrollbar without it.

**The frontend is plain HTML and JavaScript.** Phase 5 task 5.1 brings TypeScript and Vite; adding
a build toolchain here would have meant Node in CI for a page that is a setup flow and a status
report. `ui/src/bindings.ts` is generated and committed now, and becomes the only call path when
the bundler arrives.

**Pausing is asymmetric, and says so.** Transcripts are on disk, so a pause skips them and the
resume re-reads from the stored byte offset — nothing is lost. OTEL events are not replayable, so
what arrives while paused is gone; it is counted in a separate `paused_drops` rather than folded
into the backpressure counter, because one is the user's choice and the other is a shortfall.

**`toolog backfill` from the command line while the app is resident opens a second writer.** Safe
under WAL, but it contends, so the command now says so and points at the tray's *Import History*,
which uses the one writer ADR-0007 describes.
