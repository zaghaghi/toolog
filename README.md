# Toolog

> **Status: planning complete, implementation not started.** See [docs/](docs/README.md).
> Working name — see [Phase 0](docs/phases/00-foundation.md), task 0.7.

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

**OTEL** carries what transcripts never record: who approved each call and how, how long it took,
what it cost — and **the calls you denied**, which leave no transcript trace at all.

The two are joined exactly on `tool_use_id`, and where they disagree, that is a finding rather than
an inconsistency to paper over: a call only OTEL saw was rejected; a call only the transcript saw is
a gap in collection. This is how the tool demonstrates its own completeness instead of asserting it.

Neither lane puts any code on Claude Code's critical path. One is a read-only file watch; the other
is a fire-and-forget network export whose failure costs nothing.

See [ADR-0002](docs/adr/0002-dual-ingestion-transcripts-and-otel.md) for the full reasoning.

## Install

Not yet available — see [Phase 8](docs/phases/08-packaging-distribution.md).

```
brew install --cask toolog   # planned
toolog doctor --fix          # configures Claude Code telemetry, with a backup
toolog backfill              # imports your existing history
```

## Documentation

- [docs/README.md](docs/README.md) — phases, milestones, and the facts the design rests on
- [docs/adr/](docs/adr/README.md) — nine architecture decision records
- [PRIVACY.md](PRIVACY.md) — what is captured, what is not, and what never leaves
