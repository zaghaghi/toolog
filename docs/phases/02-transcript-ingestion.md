# Phase 2 — Transcript ingestion

**Goal:** `toolog backfill` fills the database from real history, and the live tailer keeps it
current. This is the content lane of ADR-0002.

**Depends on:** Phase 1. **Unblocks:** Phase 5.
**Governed by:** [ADR-0002](../adr/0002-dual-ingestion-transcripts-and-otel.md),
[ADR-0004](../adr/0004-store-raw-project-normalized.md).

## Tasks

- [x] **2.1** Streaming JSONL reader tolerant of a **partial trailing line** — files are appended to
  while being read. Track byte offsets; never emit a partially-written record.
- [x] **2.2** Envelope parser: `type`, `uuid`, `parentUuid`, `sessionId`, `promptId`, `timestamp`,
  `cwd`, `gitBranch`, `version`, `isSidechain`, `entrypoint`, `userType`,
  `sourceToolAssistantUUID`. Unknown fields are preserved in `raw_event`, never an error.
- [x] **2.3** Write every line to `raw_event` **before** parsing (ADR-0004). Projection failures must
  never lose the underlying record.
- [x] **2.4** Extract `tool_use` blocks from `assistant` records and `toolUseResult` from the paired
  `user` records; join on `tool_use_id`.
- [x] **2.5** **Per-tool normalizers** producing `input_summary`, `target_path` and `success`:
  - `Bash` — `command`, `description`; result `stdout`/`stderr`/`interrupted`/`noOutputExpected`
    (1,539 calls locally, by far the dominant case)
  - `Edit` / `Write` — `filePath`, `structuredPatch` → `file_change` rows with line counts,
    plus `originalFile` and `userModified`
  - `Read` — `filePath`, result `file`/`type`
  - `WebFetch` / `WebSearch` — `url`, `query`
  - `mcp__<server>__<tool>` — split into `mcp_server` / `mcp_tool`, `tool_kind = mcp`
  - `Agent` / `Task`, `Skill`, `ToolSearch`, `AskUserQuestion`, `ExitPlanMode`
  - [x] **A generic fallback normalizer for unknown tools. Never error, never drop.** New tools ship
    constantly; the local corpus already contains tools added across 12 versions.
- [x] **2.6** Handle `toolUseResult` as **object, bare string, and array** — all three occur in the
  local corpus (2,171 / 99 / 62). A parser assuming a single shape will fail on ~7% of real records.
- [x] **2.7** Populate `session` from envelopes, and attribute the **271 sidechain calls** to
  their subagent. ⚠️ **This task's premise was wrong and was corrected against the corpus** —
  see *Corrections* below. `agent-name` is a session label; `agentId` is the subagent.
- [x] **2.8** Handle `relocated` records — sessions whose transcript path moved. Present in the local
  corpus; ignoring them silently splits one session into two.
- [x] **2.9** `toolog backfill`: walk `~/.claude/projects/**/*.jsonl`. Resumable via stored offsets,
  reports progress, safe to re-run (dedup makes it idempotent).
- [x] **2.10** Live tail: `notify` fsevents watcher on the projects directory. Per-file byte offset;
  on truncation or inode change, **re-scan from zero** and let `content_sha256` discard duplicates.
  Handle new session files appearing and whole project directories being created.
- [x] **2.11** Debounce and coalesce watcher events — editors and Claude Code both write in bursts.

## Fixtures

- [x] **2.12** Anonymize a slice of the local 39-file corpus into `fixtures/`, spanning **all 12
  observed Claude Code versions** (2.1.161 → 2.1.259) and every `toolUseResult` shape. Replace user
  paths, hostnames, secrets and repository names; keep structure exact.
- [x] **2.13** Golden tests: fixture in → expected normalized rows out. **This is the parser's
  contract**; every future Claude Code version adds a fixture rather than a patch.

## Exit criteria

- [x] `toolog backfill` over the real corpus yields **2,475 tool calls** (the plan predicted
  ~2,334; the corpus grew while this was being built), with `Edit` **292** and `Read` **253**
  matching the plan exactly, `Write` 70 against 69, `Bash` 1,677 against 1,539 — and
  **271 sidechain calls, every one attributed**, exactly as predicted.
- [x] **Zero parse errors** across 12,938 lines; 3,941 unknown record types counted and
  reported rather than fatal.
- [x] Appending while the tailer runs produces exactly one row per tool call.
  (`tests/tail.rs::appending_while_tailing_yields_one_row_per_call`)
- [x] Killing the tailer mid-file and restarting loses nothing and duplicates nothing.
  (`tests/tail.rs::interrupting_and_resuming_loses_and_duplicates_nothing`)

## Outcome

**80 tests** workspace-wide, `clippy -D warnings` and `fmt --check` clean.

Backfill of the real corpus: 39 files, 12,938 lines, **2,475 tool calls, 30 sessions,
0 unparsable**, in 0.8 s.

### Corrections the corpus forced

**Task 2.7's premise was wrong.** The plan said to "join `agent-name` records for sidechain
attribution". Profiling showed `agent-name` maps a *session* to a label like
`host-password-reset-flow` — a worktree/agent-session name. The giveaway: a session carrying
one had 625 records, every one `isSidechain: false`. It has nothing to do with subagents.

Three separate things had been conflated, and the corpus separates them cleanly:

| Field | Meaning | Coverage |
|---|---|---|
| `agentId` | subagent **instance** | 658/658 sidechain records, 0/5,944 main-thread |
| `attributionAgent` | subagent **type** (`Explore`) | ~57% of sidechain records |
| `agent-name` | **session** label | unrelated to subagents |

`agentId` is a perfect discriminator, so it became a new column (migration 002).
`attributionAgent` is partial, so it is spread across an instance in the projector's finish
pass. The authoritative link is the spawning `Agent` call: **its result carries the same
`agentId` its sidechain records do**, and its input carries `subagent_type`.

**A real bug: relocation was not sticky.** `relocated` records name a session's new working
directory and in practice sit at the *top* of a transcript, before the records carrying the
old `cwd`. Ordinary newest-wins session merging meant the old path overwrote the new one.
Relocation is now applied as a terminal fact in `finish`, after the stream has been read.

**`toolUseResult`'s three shapes turned out to be meaningful, not just awkward:**

- **bare string** — always an error. Every one in the corpus began `"Error: "`.
- **array** — MCP content blocks, including base64 images.
- **object** — the ordinary case, keyed per tool.

### Deviations from the plan

**Task 2.12 does not scrub real transcripts.** The plan said to anonymize a slice of the
corpus. Measured first: it holds **226,192 string leaves totalling 37.3 MB of free text** —
prompts, source code, client project names. Reliably redacting that for a **public**
repository is not achievable, and one miss publishes someone's code.

Instead the fixtures are **synthetic reproductions of the observed structures**, generated
against `fixtures/schema-manifest.json` — key names and value *types* extracted from real
data with every value discarded, and verified to contain no leaf outside the type vocabulary.
Structural fidelity, zero content.

What real data still buys is breadth, so `tests/real_corpus.rs` runs the parser over
`~/.claude/projects` when it exists, asserting *properties* rather than values (zero
unparsable, every sidechain call attributed, re-projection stable) and skipping cleanly on a
machine that has never run Claude Code. Nothing from it is ever committed.

**Base64 payloads are elided from the projection**, not stored inline. A single screenshot was
the largest result in the corpus at 610 KiB. `raw_event` keeps the original, so this costs no
evidence — exactly the separation ADR-0004 exists to allow.

**The `Projector` trait gained a `finish` hook.** Relocation and subagent types can only be
settled once the whole stream has been seen.

### A note on testing against a live corpus

One test initially failed because Claude Code was **writing to the transcripts while the test
ran** — this very session appending records between two passes. The assertion, not the code,
was wrong: on a developer's own machine the corpus is live, so the invariant is that
re-ingesting creates no *duplicates*, not that a second pass stores nothing.

### Storage, re-measured with the real pipeline

76.6 MiB from a 41.8 MiB corpus (1.84x), `raw_event` 63.3%, **~31 MiB per 1,000 tool calls** —
unchanged from Phase 1. `file_change` is new and costs 980 KiB. Result bodies: median 888 B,
p95 12.0 KiB, and **28 calls over 64 KiB holding 7.0 MiB**, still the signal for Phase 7's
by-reference threshold. All queries remain sub-3 ms.
