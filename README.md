# Toolog

> **Status: v1.0 — all nine phases done.** The timeline is a virtualized list over every tool call
> with full-text search, filters that live in the URL, diffs for every `Edit`, and export to JSON,
> CSV or Markdown. The risk review runs rules written as data and drills through to the calls behind
> each finding, and can notify on refusals and high-severity rule hits, off by default. And the
> record earns its name: `toolog verify` says what it is missing and when nothing was watching,
> `toolog verify --chain` shows it has not been altered, secrets are stripped from everything the
> views show, and `toolog purge` bounds the store without deleting anything you have not seen listed
> first. It ships as one signed, notarized universal artifact, and removes itself as carefully as it
> installs: `toolog uninstall` puts `~/.claude/settings.json` back byte for byte and keeps your
> history unless you say otherwise. See [docs/](docs/README.md).
>
> **Next (v1.1, planned):** v1.0 shipped four views and two of them did not earn their place. The
> usage analytics and the live tail are gone — toolog captures cost and reports none of it
> ([ADR-0010](docs/adr/0010-no-cost-reporting.md)) — leaving three. Nothing in
> [Phases 9–11](docs/README.md) is built yet.

A local audit trail for Claude Code tool calls.

Claude Code runs tools on your machine — shell commands, file edits, network fetches. Today there is
no way to answer *"what did the agent actually do, when, in which repo, and who approved it?"*
without scrolling a terminal that has already gone.

Toolog captures every tool call, stores it in an embedded database on your machine, and gives you
three views over it: a forensic timeline, a permission and risk review, and the state of capture
itself.

## Nothing leaves your machine

The OTLP receiver binds `127.0.0.1` only. There is no analytics, no crash reporting, no remote
config, no account.

**A CI test runs a full ingest and every query the window issues, then asks the operating system
which sockets the process holds** — any address that is not loopback fails the build. A second test
proves that check can fail, by opening a socket pointed off the machine and asserting the census
sees it. The guarantee is a build failure when broken, not a promise in a readme.

**There is no exception.** ADR-0008 had reserved one — an opt-in update check — and Phase 8
declined to take it: an updater compiles an HTTP client into the binary whether its switch is on
or off, which would demote a compile-time guarantee to a runtime flag. `brew upgrade --cask
toolog` is the update path instead, and the updater plugin is named in the same test that rejects
`reqwest`. See [ADR-0008](docs/adr/0008-local-only-zero-egress.md) and [PRIVACY.md](PRIVACY.md).

## How it works

Two ingestion lanes, because neither is complete alone:

```
┌──────────────┐   OTLP/HTTP POST /v1/logs      ┌────────────────────────────┐
│ Claude Code  │ ─────────────────────────────► │  toolog (single process)   │
│              │   127.0.0.1:47318              │                            │
│  writes ───► │ ~/.claude/projects/**/*.jsonl  │  OTLP receiver             │
└──────────────┘        ▲                       │  transcript tailer         │
                        │ fsevents tail         │  normalizer + joiner       │
                        └───────────────────────┼─ SQLite (WAL + FTS5)       │
                                                │  WebView UI (3 views)      │
                                                └────────────────────────────┘
```

**Transcripts** carry the untruncated content — the full shell command, the full file diff. OTEL
truncates tool inputs at 512 characters, which is not evidence.

**OTEL** carries what transcripts never record: **who approved or refused each call, and under
which rule**, and how long it took. A refused call does appear in the transcript — but only as a
sentence inside a result body, with nothing to query. `decision` and `decision_source` exist in one
place.

The two are joined exactly on `tool_use_id`, and where they disagree, that is a finding rather than
an inconsistency to paper over: a call only the transcript saw is a gap in collection, and a call
only OTEL saw had no transcript body written. This is how the tool demonstrates its own completeness
instead of asserting it.

Neither lane puts any code on Claude Code's critical path. One is a read-only file watch; the other
is a fire-and-forget network export whose failure costs nothing.

See [ADR-0002](docs/adr/0002-dual-ingestion-transcripts-and-otel.md) for the full reasoning.

## Install

```
brew install --cask zaghaghi/tap/toolog
```

Or download the `.dmg` from [Releases](https://github.com/zaghaghi/toolog/releases). It is a
universal build signed with a Developer ID and notarized by Apple, so it opens without a
Gatekeeper warning. Verify what you got:

```
shasum -a 256 -c SHA256SUMS
spctl --assess --type execute --verbose=2 /Applications/toolog.app
```

Then, from a terminal — the same binary is both the app and the CLI:

```
toolog doctor --fix          # configures Claude Code telemetry, merged, with a backup
toolog backfill              # imports your existing history
toolog                       # starts the menu-bar app and the receiver
```

Measured on the author's machine: a 50-file, 66 MB transcript corpus mounts, copies, configures,
imports and answers its first query in about **17 seconds** of machine time — 14 s of that is
`hdiutil` verifying the disk image. The import itself is 2.0 s for 3,729 tool calls across 41
sessions.

### Uninstall

```
toolog uninstall             # show what would change
toolog uninstall --apply     # do it
```

It removes the login agent and puts `~/.claude/settings.json` **back byte for byte** from the
backup taken before toolog first wrote to it — or, if you have edited that file since, removes
only toolog's own six keys and says why it did not restore. Your recorded history is kept unless
you add `--delete-data`. `brew uninstall --cask toolog` runs the same thing; add `--zap` to
delete the history too. There is also a "Remove toolog" section on the Status page.

### From a checkout

```
npm --prefix ui ci           # the window's dependencies, once
just release                 # bundles the window, then builds one binary: the app and the CLI
just bundle                  # the distributable universal .app and .dmg
```

The window is TypeScript compiled by Vite and **embedded in the binary**, so it is built first;
`just build`, `just release`, `just run` and `just bundle` all do that for you.

## Commands

| | |
|---|---|
| `toolog` | Menu-bar app: receiver, transcript tailer, window on demand |
| `toolog doctor [--fix]` | The state of the integration. Read-only unless `--fix` |
| `toolog backfill` | Import `~/.claude/projects`. Safe to re-run |
| `toolog verify` | Cross-check the two lanes: gaps, refusals, completeness |
| `toolog verify --chain` | Walk the integrity chain over stored evidence, and print its head |
| `toolog purge` | Show what retention would remove. `--apply` to remove it |
| `toolog risk` | Evaluate the risk rules: what got approved, and how |
| `toolog export` | JSON, JSONL, CSV or Markdown, with filters |
| `toolog agent install` | A login agent so capture survives a restart |
| `toolog uninstall` | Undo the install. Restores `settings.json`; keeps your history |

`doctor --fix` writes only to `~/.claude/settings.json`, merges rather than overwrites, keeps a
timestamped backup, and refuses outright if a non-loopback OTEL endpoint is already configured —
your existing telemetry pipeline is not ours to redirect. See
[ADR-0006](docs/adr/0006-configure-via-settings-env-block.md).

## Limitations, stated rather than discovered

- **macOS only.** Linux `.AppImage`/`.deb` was a Phase 8 stretch goal and was cut, not
  attempted. Nothing in the code assumes macOS except the LaunchAgent, so the port is open —
  it would need a systemd user unit for both the install and the uninstall path.
- **Decisions and latency exist only for sessions captured live.** They arrive on the OTEL
  lane, which is not replayable: a session that ran while toolog was not listening has its
  commands and results, from the transcript, and no decision layer ever. `toolog verify`
  reports exactly which sessions those are and over what windows, rather than averaging
  across a gap as if it were not there.
- **Cost is captured and never shown.** `api_request` records are stored and counted, and
  nothing in the window or the CLI reports spend or tokens. That is a scope decision, not an
  omission — see [ADR-0010](docs/adr/0010-no-cost-reporting.md).
- **Quitting from the menu bar stops capture**, and it is meant to — the LaunchAgent restarts a
  crash but respects a deliberate exit. Anything Claude Code runs while toolog is quit is
  recoverable from transcripts by a later `toolog backfill`, minus the decision layer above.
- **No update notification.** `brew upgrade --cask toolog` covers Homebrew installs; if you
  downloaded the `.dmg` directly, nothing will tell you a new version exists. That is the
  cost of shipping an application that makes no network calls, and it was chosen with the
  cost known — see the addendum to [ADR-0008](docs/adr/0008-local-only-zero-egress.md).
- **A project directory name cannot always be decoded back to a path.** Claude Code encodes
  `/` as `-`, so `/a/b/my-app` and `/a/b/my/app` name the same directory. Project attribution
  therefore comes from `cwd` in the records; only per-project *exclusion* matches on the
  encoded name, where it is exact.
- **Anyone who can read your disk can read the database.** It is a plain SQLite file, on
  purpose — that is what lets you check the claims in `PRIVACY.md` without trusting this
  program. Encryption at rest was evaluated in Phase 7 and declined in favour of FileVault,
  with the reasoning in ADR-0008.

## Documentation

- [docs/README.md](docs/README.md) — phases, milestones, and the facts the design rests on
- [docs/adr/](docs/adr/README.md) — nine architecture decision records
- [PRIVACY.md](PRIVACY.md) — what is captured, what is not, and what never leaves
- [docs/releasing.md](docs/releasing.md) — how a signed, notarized release is cut
