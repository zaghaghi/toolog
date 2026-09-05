# Privacy

> **Skeleton.** The posture below is settled ([ADR-0008]) and is being built against.
> Sections marked *Phase 7* describe controls not yet implemented. This file is
> completed in [Phase 7](docs/phases/07-privacy-retention-integrity.md), task 7.10 —
> and it is written first deliberately, so the guarantees are stated before the code
> that must honour them.

## The short version

Nothing leaves your machine.

## What is captured

Toolog records the tool calls Claude Code makes on your machine, from two sources:

| Source | What it contributes |
|---|---|
| `~/.claude/projects/**/*.jsonl` | The tool calls themselves — shell commands, file paths, file contents read and written, diffs, and results. Read-only; toolog never modifies these files. |
| Claude Code's OpenTelemetry export | Permission decisions and their source, durations, token counts, cost, model, and **rejected calls**. Every event also carries your Claude account identity — email, user id, account and organization UUIDs — which is stored as received and never sent anywhere. |

**This is sensitive data.** It includes every shell command run in every repository,
file contents, and paths that can reveal project and client names.

## What is *not* captured

- **Your prompts.** `OTEL_LOG_USER_PROMPTS` is deliberately never set.
- **Claude's responses.** `OTEL_LOG_ASSISTANT_RESPONSES` is deliberately never set.

Only prompt *length* and invoked command *name* are recorded. See [ADR-0006] for the
exact environment block toolog writes, and why it uses per-signal variables so an
existing corporate telemetry pipeline is left untouched.

## Where it is stored

One SQLite file, on your machine:

```
~/Library/Application Support/toolog/toolog.db
```

Readable with any `sqlite3` binary. Deleting it deletes everything toolog holds.

Alongside it, `rules.toml` — if you write one — holds your own risk rules. It is read, never
written, and is the file to edit to retune or switch off anything `toolog risk` reports.

Also alongside it, `prefs.json` holds the switches you have turned on. It is created the first time
you turn one on; until then there is no file, because **every switch is off by default**. It
contains three booleans and nothing else — no history, no identifiers.

## Secrets

Claude Code runs shell commands, and shell commands carry keys. A tool that keeps a durable record
of what ran would, left alone, keep a durable record of every credential that went past.

**Secrets are removed from the projection** — the rows the timeline, risk and usage views read, and
what an export contains. API keys, bearer tokens, `Authorization` headers, private keys, passwords
in connection strings, AWS and Google credentials, JWTs. The pattern set lives in the binary and is
listed in `crates/toolog-core/src/redact/default.toml`; a `redaction.toml` beside the database adds
to it, or replaces any pattern by id.

**The evidence store is not redacted, unless you ask.** `raw_event` holds every record exactly as it
arrived, because that is what every other table is rebuilt from: a pattern that turns out to be
wrong can be fixed and the projection regenerated, but only while the original is still there. The
cost is real and stated rather than hidden — with the default, a secret that went past is on disk in
`raw_event`.

The switch is in **Status → Privacy**. Turning it on redacts records **before** they are stored, and
that is irreversible in two directions: those records never hold the original, and it does not reach
backwards — anything already stored keeps what it holds. Deleting the database is the blunt way to
be rid of that; a finer one arrives with retention (Phase 7.4).

Redaction is deliberately over-eager: a row reading `[redacted: env-assignment]` where a variable
was merely *named* like a secret loses a little fidelity, while a row printing a live key loses the
key. To see what the patterns would do to your own store before changing anything:

```
cargo run --release -p toolog-core --example measure_redaction -- ~/Library/Application\ Support/toolog/toolog.db
```

It reads, reports and writes nothing.

## What leaves your machine

Nothing — with one exception, named here rather than buried.

- The OTLP receiver binds `127.0.0.1` only. It is not reachable from your network.
- There is no analytics, no crash reporting, no remote configuration, no account, no
  license check.
- **A CI test asserts that no non-loopback socket is opened during a full ingest and
  query run.** The guarantee is a build failure when broken, not a promise in a
  document. *(Phase 7, task 7.7.)*

**Exports go where you point them.** The timeline's export opens a native save panel, and toolog
writes the file you choose and nothing else. What it contains is the same sensitive data the store
holds — commands, paths, results — so where you put it is the decision that matters.

**The exception:** an update check against GitHub Releases. It is **off by default**,
**opt-in at first run**, and **sends no user data** — it fetches a version manifest.
*(Phase 8, task 8.5.)*

## Your controls — *Phase 7*

- Pause and resume capture from the menu bar
- Per-project exclusion
- Retention limits by age and size, with a preview of exactly what a purge deletes
- Delete a session, removing raw and derived records together
- Secret redaction — API keys, tokens, private keys, `.env` values
- A documented choice about whether redaction also rewrites the raw evidence store,
  since redacting evidence is irreversible and that trade-off is yours to make

## Uninstalling — *Phase 8*

Uninstall removes the LaunchAgent, restores `~/.claude/settings.json` from the backup
taken before toolog modified it, and asks before deleting the database — defaulting to
keeping it.

[ADR-0006]: docs/adr/0006-configure-via-settings-env-block.md
[ADR-0008]: docs/adr/0008-local-only-zero-egress.md
