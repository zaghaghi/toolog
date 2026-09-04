# Toolog

> **Status: Phases 0–4 done.** Capture works end to end — both lanes, the menu-bar app and the
> CLI. The timeline and the other three views arrive in Phases 5 and 6.
> See [docs/](docs/README.md).

A local audit trail for Claude Code tool calls.

Claude Code runs tools on your machine — shell commands, file edits, network fetches. Today there is
no way to answer *"what did the agent actually do, when, in which repo, and who approved it?"*
without scrolling a terminal that has already gone.

Toolog captures every tool call, stores it in an embedded database on your machine, and gives you
four views over it: a forensic timeline, permission and risk review, usage analytics, and live
session monitoring.

## Nothing leaves your machine

The OTLP receiver binds `127.0.0.1` only. There is no analytics, no crash reporting, no remote
config, no account. A CI test asserts that no non-loopback socket is ever opened — the guarantee is
a build failure when broken, not a promise in a readme.

The one exception, named plainly: an **opt-in, off-by-default** update check against GitHub
Releases, which sends no user data. See [ADR-0008](docs/adr/0008-local-only-zero-egress.md) and
[PRIVACY.md](PRIVACY.md).

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
                                                │  WebView UI (4 views)      │
                                                └────────────────────────────┘
```

**Transcripts** carry the untruncated content — the full shell command, the full file diff. OTEL
truncates tool inputs at 512 characters, which is not evidence.

**OTEL** carries what transcripts never record: **who approved or refused each call, and under
which rule**, how long it took, and what it cost. A refused call does appear in the transcript — but
only as a sentence inside a result body, with nothing to query. `decision` and `decision_source`
exist in one place.

The two are joined exactly on `tool_use_id`, and where they disagree, that is a finding rather than
an inconsistency to paper over: a call only the transcript saw is a gap in collection, and a call
only OTEL saw had no transcript body written. This is how the tool demonstrates its own completeness
instead of asserting it.

Neither lane puts any code on Claude Code's critical path. One is a read-only file watch; the other
is a fire-and-forget network export whose failure costs nothing.

See [ADR-0002](docs/adr/0002-dual-ingestion-transcripts-and-otel.md) for the full reasoning.

## Install

No packaged build yet — see [Phase 8](docs/phases/08-packaging-distribution.md). From a checkout:

```
cargo build --release        # one binary: the app and the CLI
toolog doctor                # what is configured, what is running, what is missing
toolog doctor --fix          # configures Claude Code telemetry, merged, with a backup
toolog backfill              # imports your existing history
toolog                       # starts the menu-bar app and the receiver
```

`brew install --cask toolog` is the Phase 8 target.

## Commands

| | |
|---|---|
| `toolog` | Menu-bar app: receiver, transcript tailer, window on demand |
| `toolog doctor [--fix]` | The state of the integration. Read-only unless `--fix` |
| `toolog backfill` | Import `~/.claude/projects`. Safe to re-run |
| `toolog verify` | Cross-check the two lanes: gaps, refusals, completeness |
| `toolog export` | JSON, JSONL or CSV, with filters |
| `toolog agent install` | A login agent so capture survives a restart |

`doctor --fix` writes only to `~/.claude/settings.json`, merges rather than overwrites, keeps a
timestamped backup, and refuses outright if a non-loopback OTEL endpoint is already configured —
your existing telemetry pipeline is not ours to redirect. See
[ADR-0006](docs/adr/0006-configure-via-settings-env-block.md).

## Documentation

- [docs/README.md](docs/README.md) — phases, milestones, and the facts the design rests on
- [docs/adr/](docs/adr/README.md) — nine architecture decision records
- [PRIVACY.md](PRIVACY.md) — what is captured, what is not, and what never leaves
