# Phase 7 — Privacy, retention & integrity

**Goal:** earn the word "audit". Until this phase the tool records; after it, the record can be
trusted, bounded and defended.

**Depends on:** Phases 2, 3, 6. **Unblocks:** Phase 8.
**Governed by:** [ADR-0008](../adr/0008-local-only-zero-egress.md),
[ADR-0009](../adr/0009-correlate-on-tool-use-id.md).

## Tasks

- [x] **7.1** **`toolog verify` — reconciliation.** Cross-check the lanes per ADR-0009:
  - OTEL-only calls → **rejected calls**, listed as such
  - Transcript-only calls → **collection gaps**, with the window in which the app was not running
  - A per-session completeness figure

  This is what lets the tool state its own completeness rather than assume it, and it is the
  feature that most distinguishes an audit tool from a log viewer.
- [x] **7.2** **Secret redaction** at normalization: API keys, bearer tokens, `Authorization`
  headers, private keys, `.env` values, connection strings, AWS/GCP credentials. Pattern set as
  data, user-extensible.
- [x] **7.3** **Decide, expose and document whether `raw_event` is redacted too.** Redacting the
  evidence store is defensible but lossy and irreversible; leaving it intact is defensible but
  keeps secrets on disk. This is the user's call, surfaced explicitly in Preferences with the
  trade-off stated — not a silent default.
- [ ] **7.4** Retention policy: age cap and size cap, with a **purge preview showing exactly what
  would be deleted** before anything is. `VACUUM` after purge.
- [ ] **7.5** Oversized result bodies stored by reference beyond a threshold, using the Phase 1.9
  measurements to set it.
- [x] **7.6** **Integrity hash-chain** over `raw_event`: each row hashes its predecessor, making
  post-hoc tampering detectable. `toolog verify --chain` walks and reports.
  Document the honest limit: this detects modification of stored evidence, it does not prove the
  evidence was complete when written — that is what 7.1 is for.
- [x] **7.7** **CI egress test** (ADR-0008): assert **no non-loopback socket is opened** during a
  full ingest plus UI-query run. The privacy guarantee must be a build failure when broken, not a
  convention.
- [ ] **7.8** Pause/resume capture from the tray; per-project exclusion list; delete-a-session
  removing raw and projected rows together (and breaking the hash chain visibly rather than
  silently re-chaining).
- [ ] **7.9** Evaluate **SQLCipher** for encryption at rest; decide and record as an ADR addendum.
  State the honest trade-off: the key must live locally, so this mainly defends the stolen-disk and
  backup cases, not a compromised logged-in machine.
- [ ] **7.10** Complete `PRIVACY.md`: exactly what is stored, where, what never leaves, and the one
  named exception (the opt-in update check, Phase 8).

## Progress

**Done: 7.1, 7.2, 7.3, 7.6, 7.7.** The record can now say what it is missing, show that it has not
been altered, keep secrets out of what it shows, and fail the build if anything tries to leave the
machine. Retention, oversized bodies, session deletion and the SQLCipher decision are next.

### Completeness and integrity are different claims

`toolog verify` answers *what is missing*; `toolog verify --chain` answers *what has changed*.
Neither implies the other, and the code says so in both places: a record that was never captured
leaves nothing to tamper with, and a chain that checks out says nothing about what was never
written.

**Completeness (7.1).** The lanes are reconciled per session and over time. A session reports how
much of its *approval* layer survives — deliberately not called "completeness" on its own, because
the content layer is complete for those calls; what is missing is who allowed them. Windows with no
decision layer are found with a gaps-and-islands pass over call order and reported machine-wide,
because "capture was not running" is a fact about the machine rather than about a session.

On the owner's store: **17.5% of 3,383 calls have their approval on record.** One 89-day window
holds 2,672 of the rest — history imported from before toolog ran — and the small windows after it
are the minutes the application was stopped during this phase's own development, which is the
feature working on its author.

**Integrity (7.6).** Every `raw_event` row carries `chain_sha256`, computed in the same statement
that stores it, over a digest of the row linked to the value before it. Ordering is safe because one
process holds one write connection (ADR-0007). Rows written before the chain existed are sealed by
whatever owns writes next — the application at startup, or a backfill — and sealing in bulk produces
the same chain as sealing on arrival, which is asserted.

The honest limit is documented where the code is, not only here. Walking detects any edit that
leaves the rest of the chain alone; it cannot detect a rewrite that re-seals everything after the
edit, because such a chain is consistent with itself. That is what the printed **head** is for: a
single string covering every record before it, worth keeping outside the database. And deleting a
record from the middle leaves the head untouched — so neither check substitutes for the other.

### Redaction, and what measuring it changed (7.2, 7.3)

Patterns are data, the same shape as the risk rules: a built-in TOML set and a user file whose ids
replace built-ins. A pattern that does not compile is skipped with a warning — a typo in a regex
must not stop capture, because a tool that stops recording when misconfigured records nothing.

**The projection is redacted; the evidence is not, unless asked.** That is 7.3's decision, and it
follows from ADR-0004: `raw_event` is what every other table is rebuilt from, so redacting it is
irreversible in a way redacting a projection is not — a pattern that turns out to be wrong can be
fixed and the projection regenerated, but only while the original is there. The cost is stated
rather than hidden, in **Status → Privacy** and in PRIVACY.md: with the default, a secret that went
past is on disk in `raw_event`. Turning the switch on is forward-only; it cannot reach backwards.

Two implementation notes worth keeping:

- **Applying patterns one after another was wrong.** The first replacement leaves
  `password=[redacted: password-assignment]` in the text, and the next pattern reads `[redacted:`
  as the value of `password=` and redacts *that*. Every pattern now matches the original in one
  pass, with earlier patterns winning overlaps, and existing markers protected — which is also what
  makes it idempotent, and it has to be, because with `redact_evidence` on the projector reads
  bodies that were already redacted on the way in.
- **Redaction happens at normalization**, at the one point every summary, target and result body
  passes through, so a new tool's normalizer cannot forget it.

#### The pattern set was wrong until it was measured

`cargo run --example measure_redaction` reads a store read-only and reports what each pattern would
change, with the text around every change. Run against the owner's 3,497 commands and 3,496 result
bodies, the first draft produced **five distinct false positives**, every one of which is now a
regression test:

| What it mangled | Why | Fix |
|---|---|---|
| `TOKEN=$(curl -s …)` → `TOKEN=[redacted] -s …` | a command substitution is an expression, not a literal | value may not start with `$` or a backtick |
| `let token = issue_jwt(host_id, "host")` | source code in a heredoc; a shell assignment has no spaces around `=` | no space before `=` |
| `pub password: String,` (×18) | a type name is not a password | the `=` form only; the colon form must be quoted |
| `"new-password" : "current-password"` | matched on the *tail* of a quoted string | the key must be fully quoted, or start a line |
| ``Authorization: Bearer`. Design tokens…`` (×12) | prose describing an API | the token must be 20+ characters, so a bare scheme word is not one |

After tuning: **8 of 3,497 commands** and **31 of 3,496 result bodies**, with no false positives
left. What it does catch on the real store includes a live-shaped JWT in an API response and an AWS
STS access key in `aws sts` output — both exactly what this is for.

A detail that says something about doing this on your own machine: several remaining "hits" turned
out to be the measurement's *own previous output*, stored in the timeline because running it is a
tool call like any other. The corpus contains the tool's development.

### Zero egress is now a build failure (7.7)

Three checks, because each catches what the others cannot: a **socket census** taken from the
operating system after a full ingest and every query the window issues; a **manifest check** that
nothing here asks for an HTTP client; and a **source check** that nothing hand-rolls a connection on
`std::net`.

A fourth test, in its own binary, proves the census can fail — it opens a UDP socket with a
non-loopback peer and asserts the census sees it. It has to be a separate process: the census asks
what *this* process holds, so a socket opened to test it would be counted by it.

Two things fell out of writing it:

- **`reqwest` is in `Cargo.lock`,** declared optionally by `tauri` behind a feature this workspace
  does not enable. `cargo tree -e normal` for the host shows no `reqwest`, no `hyper-tls`, no
  `rustls`: it is listed, not linked. So the manifest check reads the manifests — the thing we
  control and can state exactly — and leaves "does anything transitively reach for one" to the
  census, which watches rather than reads.
- **The health probe could have connected anywhere.** `toolog-otlp`'s `probe` is the one outbound
  connection in the workspace, and it took whatever `SocketAddr` it was handed. It now refuses
  anything that is not loopback, with its own test asserting it refuses *without connecting* — so
  the egress test can skip that file on a property rather than on trust.

## Exit criteria

- `toolog verify` reports 100% reconciliation on accepted calls with rejections listed separately.
  **Met, and the number on a real store is 17.5% — which is the point.** Reconciliation is reported,
  not assumed: every call the OTLP lane witnessed is accounted for, the 2,790 it did not are named
  with the windows they fall in, and the 2 refusals are listed separately from both.
- A deliberately introduced secret in a test transcript is redacted in the projection. **Met**, in
  `crates/toolog-ingest/tests/redaction.rs` — and its other half is asserted too: by default the
  evidence still holds it, which is the trade-off 7.3 exists to make explicit.
- `toolog verify --chain` detects a row edited directly with the `sqlite3` CLI. **Met on the real
  corpus**: a `.backup` copy of the owner's store sealed 17,633 records, an `UPDATE` from `sqlite3`
  appended one byte to one body, and the walk reported exactly that row with exit code 1.
- The CI egress test passes with zero non-loopback sockets, and fails if a stray HTTP call is added.
  **Met**, and the "fails if" half is a test of its own rather than a claim.
