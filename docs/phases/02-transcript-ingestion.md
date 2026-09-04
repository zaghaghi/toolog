# Phase 2 — Transcript ingestion

**Goal:** `toolog backfill` fills the database from real history, and the live tailer keeps it
current. This is the content lane of ADR-0002.

**Depends on:** Phase 1. **Unblocks:** Phase 5.
**Governed by:** [ADR-0002](../adr/0002-dual-ingestion-transcripts-and-otel.md),
[ADR-0004](../adr/0004-store-raw-project-normalized.md).

## Tasks

- [ ] **2.1** Streaming JSONL reader tolerant of a **partial trailing line** — files are appended to
  while being read. Track byte offsets; never emit a partially-written record.
- [ ] **2.2** Envelope parser: `type`, `uuid`, `parentUuid`, `sessionId`, `promptId`, `timestamp`,
  `cwd`, `gitBranch`, `version`, `isSidechain`, `entrypoint`, `userType`,
  `sourceToolAssistantUUID`. Unknown fields are preserved in `raw_event`, never an error.
- [ ] **2.3** Write every line to `raw_event` **before** parsing (ADR-0004). Projection failures must
  never lose the underlying record.
- [ ] **2.4** Extract `tool_use` blocks from `assistant` records and `toolUseResult` from the paired
  `user` records; join on `tool_use_id`.
- [ ] **2.5** **Per-tool normalizers** producing `input_summary`, `target_path` and `success`:
  - `Bash` — `command`, `description`; result `stdout`/`stderr`/`interrupted`/`noOutputExpected`
    (1,539 calls locally, by far the dominant case)
  - `Edit` / `Write` — `filePath`, `structuredPatch` → `file_change` rows with line counts,
    plus `originalFile` and `userModified`
  - `Read` — `filePath`, result `file`/`type`
  - `WebFetch` / `WebSearch` — `url`, `query`
  - `mcp__<server>__<tool>` — split into `mcp_server` / `mcp_tool`, `tool_kind = mcp`
  - `Agent` / `Task`, `Skill`, `ToolSearch`, `AskUserQuestion`, `ExitPlanMode`
  - [ ] **A generic fallback normalizer for unknown tools. Never error, never drop.** New tools ship
    constantly; the local corpus already contains tools added across 12 versions.
- [ ] **2.6** Handle `toolUseResult` as **object, bare string, and array** — all three occur in the
  local corpus (2,171 / 99 / 62). A parser assuming a single shape will fail on ~7% of real records.
- [ ] **2.7** Populate `session` from envelopes. Join `agent-name` records
  (`sessionId` → `agentName`) so the **271 sidechain calls** are attributed to their subagent rather
  than the main thread.
- [ ] **2.8** Handle `relocated` records — sessions whose transcript path moved. Present in the local
  corpus; ignoring them silently splits one session into two.
- [ ] **2.9** `toolog backfill`: walk `~/.claude/projects/**/*.jsonl`. Resumable via stored offsets,
  reports progress, safe to re-run (dedup makes it idempotent).
- [ ] **2.10** Live tail: `notify` fsevents watcher on the projects directory. Per-file byte offset;
  on truncation or inode change, **re-scan from zero** and let `content_sha256` discard duplicates.
  Handle new session files appearing and whole project directories being created.
- [ ] **2.11** Debounce and coalesce watcher events — editors and Claude Code both write in bursts.

## Fixtures

- [ ] **2.12** Anonymize a slice of the local 39-file corpus into `fixtures/`, spanning **all 12
  observed Claude Code versions** (2.1.161 → 2.1.259) and every `toolUseResult` shape. Replace user
  paths, hostnames, secrets and repository names; keep structure exact.
- [ ] **2.13** Golden tests: fixture in → expected normalized rows out. **This is the parser's
  contract**; every future Claude Code version adds a fixture rather than a patch.

## Exit criteria

- `toolog backfill` over the real corpus yields **~2,334 tool calls**, with `Bash` ≈ 1,539,
  `Edit` ≈ 292, `Read` ≈ 253, `Write` ≈ 69, and **271 sidechain calls attributed to named agents**.
- Zero parse errors; unknown record types counted and reported, not fatal.
- Appending to a transcript while the tailer runs produces exactly one new row per tool call.
- Killing the tailer mid-file and restarting loses nothing and duplicates nothing.
