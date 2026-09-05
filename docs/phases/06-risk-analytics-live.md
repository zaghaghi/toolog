# Phase 6 — Risk, analytics & live views

**Goal:** the remaining three of the four surfaces the owner asked for, built on Phase 5's
infrastructure.

**Depends on:** Phases 3, 4, 5. **Unblocks:** Phase 7 (rules inform redaction).

## Risk & permission review

*"What got approved, and how?"* — the view only the OTEL lane makes possible (ADR-0002).

- [x] **6.1** Rule engine over `tool_call`. **Rules as data, not code**, so new rules ship without a
  release and users can add their own.
- [x] **6.2** Starter rule set:
  - Sessions run under `bypassPermissions` or `dontAsk`
  - Writes outside the session `cwd` or outside any known project root
  - **Auto-approved destructive Bash** (`decision_source = config`): `rm -rf`, `dd`, `mkfs`,
    `curl … | sh`, `git push --force`, history rewrites
  - **Rejected-then-retried-and-accepted** sequences — the agent working around a refusal, and the
    pattern that most justifies this tool existing
  - Secrets read: `.env`, `id_rsa`, `.pem`, `credentials`, keychain access
  - Network-reaching Bash commands (`curl`, `wget`, `nc`, `ssh`, `scp`)
  - MCP tool usage grouped by server
  - Tool calls during a session where permission mode changed mid-flight
- [x] **6.3** Severity ranking, drill-through from finding to the underlying calls, and
  dismiss-with-note (dismissals stored, never deleting the underlying row).
- [x] **6.4** Per-project risk summary — the "posture" glance.

## Usage analytics

- [x] **6.5** Aggregates from `api_request` + `tool_call`: cost and tokens by project / session /
  model / day, tool frequency, error rate by tool, p50/p95 duration, cache-hit ratio, sidechain
  share, active time.
- [x] **6.6** Charts. **Load the `dataviz` skill before writing any chart code** — palette,
  accessibility and dark-mode behaviour are decided there, not ad hoc.
- [x] **6.7** Date-range comparison (this week vs last) and cost-per-project leaderboard.
- [x] **6.8** Honest empty states: cost data exists only for sessions captured live by the OTLP lane,
  never for backfilled history. **Say so in the UI** rather than rendering a misleading zero.

## Live monitoring

- [x] **6.9** Ingest → Tauri event → UI channel for `live_tool_call`. In-process, no IPC (ADR-0007).
- [x] **6.10** Concurrent sessions as parallel lanes, each with a running cost meter and current tool.
- [x] **6.11** Idle/active indication per session; auto-scroll with a pause-on-interaction affordance.
- [x] **6.12** Native notifications on rejected calls and high-severity rule hits, off by default and
  individually toggleable.

## Outcome

**Done: all twelve tasks.** The three remaining views are built on Phase 5's infrastructure, and
the backend each of them needs lives in `toolog-core` with the rest of the SQL (ADR-0003):
`rules` for the risk layer, `analytics` for usage and the live lanes. 249 Rust tests and 122
frontend tests; `just check` is clean.

Two command-line entry points came with them, so every number in the window can be checked from a
terminal: `toolog risk` and `toolog usage [--days N] [--project PATH]`.

### The risk engine and its rules (6.1, 6.2)

`toolog-core::rules` compiles a TOML rule into bound SQL; `toolog risk` runs it from the command
line. 17 tests in `crates/toolog-core/tests/rules.rs`, plus the permission-mode work below.

### `permission_mode` comes from the transcript, not from OTEL

[ADR-0002](../adr/0002-dual-ingestion-transcripts-and-otel.md) lists `permission_mode_changed` among
the events OTEL emits, and [ADR-0009](../adr/0009-correlate-on-tool-use-id.md) gave the OTLP lane the
`permission_mode` column. Task 6.2's first two rules are about the permission mode, so the first
thing this phase did was look at what actually arrives.

**987 OTLP records across 11 event types, and not one `permission_mode` attribute.** No
`permission_mode_changed` event either. The only mode-shaped attribute Claude Code 2.1.260 sends is
`safe_mode`, a boolean, and it was `false` on all 364 records carrying it. The column was null for
every one of the 3,023 calls in the owner's store, which is why the Phase 5 filter for it could never
offer a value.

The transcript carries it, in two shapes: a dedicated `permission-mode` record, and a
`permissionMode` field on each `user` turn. Real values: `auto`, `plan`, `acceptEdits`, `default`,
`dontAsk`.

So the column changed lanes. `TranscriptFacts` owns it, `OtelFacts` no longer has the field at all,
and the transcript projector follows the mode as records go by, stamping each call with the mode in
force and writing a `permission_mode_change` row where it moves. After re-projecting the owner's
store: **3,023 of 3,023 calls carry a mode, and 55 changes are recorded** in a table that had been
empty since Phase 3.

Two details worth keeping:

- **A bare `permission-mode` record has no uuid and no timestamp**, so two of them with the same
  value are byte-identical and the second is dropped by content-hash deduplication (ADR-0004) — an
  `auto` → `plan` → `auto` return loses its last step. Every `user` record is unique, so the next
  prompt turn resynchronises the mode. Both sources are read for that reason.
- **A subagent's transcript is a separate file carrying no `permission-mode` record**, which left
  271 sidechain calls unstamped. `inherit_permission_modes` fills those from the session's own
  recorded changes, preferring the latest at or before the call. It is the only inference in the
  projection, it fills nulls only, and it says so where it is written.

The same pass confirmed the privacy posture by measurement rather than assumption: `prompt` and
`response` attributes **are** present on OTLP records, and their value is the literal string
`<REDACTED>`, because ADR-0008 never sets `OTEL_LOG_USER_PROMPTS` or `OTEL_LOG_ASSISTANT_RESPONSES`.

The re-projection was verified lossless on an exact copy before being run on the live store:
decisions 224 → 224, durations 222 → 222, api\_requests 220 → 220, cost identical to the micro-dollar,
file changes 384 → 384.

### What the rules found, and the two bugs that finding them exposed

Running the starter set against the real corpus is the only way to know whether a rule set is worth
having. The first run reported 2,432 calls for one rule and flagged two calls that had never run a
destructive command. Both were bugs in the engine, not in the data:

- **A heredoc body is not the command.** `cat > notes.md <<'EOF' … rm -rf … EOF` puts its whole body
  in `input_summary`, so a rule looking for `rm -rf` flagged a call that was *writing documentation
  about* `rm -rf`, and a `.env` rule flagged one writing a `.gitignore` that mentions it. Rules can
  now match `first_line` only, and the command-shaped ones do. Auto-approved destructive commands
  went from 2 findings to 0 — both were this.
- **`_` is a `LIKE` wildcard.** The escaping added backslashes without an `ESCAPE` clause, so
  `id_rsa` compiled to a pattern that could only match a string containing a backslash — it could
  never have fired. Credential findings fell from 17 to 5 once both bugs were fixed, and the five
  that remain are real.
- **A session-scoped rule is about the session.** "Permission mode changed mid-session" listed every
  call in every such session: 2,432 rows, true and useless. It now reports the 14 sessions.

### The risk view (6.3, 6.4)

A review has to survive the reader disagreeing with it, which decided two things. **A dismissal
hides nothing**: the finding keeps its place in the list, greyed, carrying the note; what changes is
the per-project posture, because that is a claim about what still needs answering. And a dismissal
**requires a reason** — a dismissal with no reason is just a hidden finding, which is the thing this
view exists not to have.

The drill-through fetches the rule's own matches (`rules::calls`) rather than translating a rule
into a `TimelineFilter`. It has to: `outside_cwd` and `first_line` are not columns, so a filter
would quietly show a *similar* set of calls, which is worse than none. Any one of them opens in the
timeline, where the evidence is.

### Usage analytics (6.5–6.8)

`toolog-core::analytics` answers each question from the table that can actually answer it, and says
which that was. `tool_call` is written by both lanes, so call counts, error rates, latency and
active time cover the whole corpus. `api_request` is OTLP-only, so cost and tokens do not — and
every aggregate therefore carries a `Coverage`, with `measured` and `complete` as fields rather than
something the caller has to work out. Task 6.8 is that field reaching the interface: **spend with no
cost data reads "not captured", never `$0.00`**, and a banner says how many sessions were captured
live and why the rest never will be.

Definitions worth pinning, because each could reasonably have been something else:

- **Active time** is wall-clock with a tool call at least every five minutes, summed per session. A
  session where someone read a diff for an hour is two active stretches, not an hour of work: the
  store cannot see the reading, and counting the gap would invent time nobody observed.
- **Error rate** divides by calls with a *recorded outcome*, not by all calls. A call only OTEL
  witnessed has no `success` at all, and counting it as a success flatters the rate.
- **Cache-hit ratio** counts cache creation as input, because it was billed as input.
- **Days are the reader's days.** The window carries a UTC offset and the SQL shifts the timestamp
  before taking the date; bucketing in UTC would put an evening's work on tomorrow's bar for anyone
  east of Greenwich.
- **A model breakdown counts requests, not calls.** A tool call is not made by a model; it is made
  in a turn a model asked for. Its call column is legitimately zero.

### Charts (6.6)

Built to the `dataviz` skill, and the procedure changed the design twice. Every chart plots **one
measure** — calls and spend are different scales, so they are two charts rather than one with two
y-axes, which on an audit tool would let a reader see a correlation the data does not contain. And
because colour's only job here is magnitude, there is **one hue and no legend**; what would have
been a four-colour outcome breakdown became three stat tiles instead, which is what the skill's own
heuristic says to do when the story is a number.

The palette is not eyeballed. Four values — a mark and a meter track, in each mode — were run
through the skill's validator against the real surfaces and pass all six checks: lightness band,
chroma floor, CVD separation, normal-vision floor, contrast. The dark-mode mark had to move from
`--clay`'s `#d97757` (L 0.672, just outside the 0.48–0.67 band) to `#d0694b`.

`docs/design/analytics.html` links both real stylesheets, the way the Phase 5 mockup does, so the
chart CSS can be checked in a browser without driving the application.

### Live monitoring (6.9–6.12)

**6.9 replaced the poll with a real channel.** Phase 4 emitted `live_tool_call` from a 500 ms timer
over a rowid cursor, with a comment saying Phase 6 would fix it. It now comes from SQLite's update
hook on the writer's connection: every row either lane writes passes through that one connection, so
one hook covers both without threading a callback through each projector. Rowids are collected during
a job and delivered **after** it commits — the hook fires before the commit, and a reader told about
a row that early would look for one it cannot see yet.

The channel delivers *updates* as well as inserts, which is the point: a call the transcript created
and OTEL later completed is the same row twice, and the second arrival is where the duration and the
decision come from. Both the timeline's "new calls" counter and the live feed therefore key on
`tool_use_id` — the counter used to say "3 new calls" for one command.

**Nothing in the live view says a session ended.** The store has no such record; Claude Code does not
announce one. A lane goes *idle* after two minutes without a call and drops off after thirty. "Idle"
is an observation; "finished" would be a guess.

**Notifications are off** (6.12). Both switches default off, are individually toggleable, and live in
`prefs.json` beside the database so the resident process can act on them and they survive a restart.
The policy — a refusal wins over a rule hit, and the rules are only consulted when that switch is on
— is a pure function with its own tests, so it is checkable without a window.

## What running it on real data found

### The Content Security Policy silently discards inline styles

The first build of the usage view drew **thirty-one hairlines under a y-axis that went to 800**. The
unit tests passed, and the same markup and CSS rendered correctly in Chrome.

The window runs under `style-src 'self'`, so a `style` **attribute** is dropped — silently, with the
element rendering at its natural size. Phase 5's virtualized list is unaffected because it assigns
`node.style.top` through the CSSOM, which CSP does not govern; the charts used `setAttribute`. Every
bar width, column height and meter fill was affected, and nothing anywhere reported an error.

`el` now takes a `style` option that assigns through the CSSOM, and a test asserts every mark
carries a size. Two wrong diagnoses came first — a percentage height on a flex item, then percentage
offsets — and both are recorded in the CSS comment, because "correct in Chrome, wrong in the app"
is the shape of the next bug of this kind too.

### A tenfold change is not a percentage anyone reads

The owner's store has one heavily-captured month and almost nothing before it, so the honest
comparison against the preceding period was `↑ 9658%`. True and useless. Above tenfold the delta
now reads as a multiple (`↑ ×98.5`).

### A call captured after a restart had no permission mode

The transcript projector follows the mode as records go by, and on the live path it starts empty —
so a session that had been running for an hour before the process started had *no* mode for every
call captured afterwards. The live view showed it as `—` for a session the store could perfectly
well account for. `project::last_known_mode` recovers it with one indexed lookup, cached per session;
it is the same inference `inherit_permission_modes` makes at the end of a backfill, available on the
live path where a whole-table update would be far too heavy.

### The tail tests could not all run at once

Not this phase's code, but this phase's `just check` is what exposed it. The six Phase 2 live-tail
tests each spin a real filesystem watcher, and six at once contend for FSEvents badly enough that one
sees *no* events inside its ten-second budget — an empty store and a confusing failure. They pass
individually, and passed in CI by luck. A process-wide mutex now runs them one at a time: about two
seconds for all six, and no flake in repeated runs.

## Exit criteria

- **The risk view flags a deliberately auto-approved `rm -rf`, with drill-through to the exact
  call.** Met, and it is the first test in `crates/toolog-core/tests/rules.rs` and the first in
  `ui/src/risk.test.ts`. On the owner's real store the same rule correctly reports *nothing*: both
  candidates were heredoc bodies documenting `rm -rf` rather than running it.
- **Analytics totals reconcile against `claude_code.api_request` costs.** Met, exactly.
  `toolog usage` reported $76.22 over 420 requests and 113,801,231 tokens; summing `api_request`
  directly in `sqlite3` gives 76,221,173 micro-dollars over 420 rows and the same token total.
  (A detail the reconciliation surfaced: five sessions have `api_request` rows but one of them has no
  tool calls at all, which is why coverage reads "4 of 28 sessions" rather than five.)
- **Two concurrent Claude Code sessions appear as separate live lanes with correct attribution.**
  Verified in tests — one over a store with two sessions in `crates/toolog-core/tests/analytics.rs`,
  one over the rendered view in `ui/src/live.test.ts` — and with **one** real session live in the
  window, correctly attributed to its project, branch, mode and running cost. Not verified with two
  real sessions at once: doing that on demand would have meant writing synthetic transcripts into the
  owner's own store, and fabricating audit records is the one thing this tool must never do.

## Not verified

- **The notification banner itself.** The decision to notify is tested; showing it needs macOS
  notification permission, which prompts the user, and both switches are off by default so nothing
  asked for it. Turning one on is what grants it.
- **Light mode.** The tokens are defined for both modes and the palette passes the validator against
  both surfaces, but every screenshot in this phase was taken in dark mode.
