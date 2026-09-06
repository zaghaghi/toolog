# Privacy

> Every control described here is built and tested. It was written before the code, so the
> guarantees were stated before anything had to honour them; where a claim is enforced by a
> test, this says which. The one thing still ahead is uninstall, in
> [Phase 8](docs/phases/08-packaging-distribution.md).

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

**Secrets are removed from the projection** — the rows the timeline and risk views read, and
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

Nothing. Not conditionally, not by default, not unless you turn something on.

- The OTLP receiver binds `127.0.0.1` only. It is not reachable from your network.
- There is no analytics, no crash reporting, no remote configuration, no account, no
  license check.
- **A CI test runs a full ingest and every query the window issues, then asks the operating
  system which sockets this process holds.** Any address that is not loopback fails the
  build. Two further tests close what a census cannot see: nothing in this workspace asks
  for an HTTP client, and no source file opens a connection on `std::net`. A fourth test
  proves the census can fail, by opening a socket pointed off the machine and asserting it
  is seen — a check that cannot fail is decoration.
- The one place that does connect is the health probe that asks *our own* receiver whether
  it is running, and it **refuses any address that is not loopback**, with a test asserting
  it refuses before connecting.

**Exports go where you point them.** The timeline's export opens a native save panel, and toolog
writes the file you choose and nothing else. What it contains is the same sensitive data the store
holds — commands, paths, results — so where you put it is the decision that matters.

**There is no exception.** An earlier draft of this file reserved one — an opt-in update
check against GitHub Releases, which ADR-0008 had permitted. Phase 8 evaluated it and
declined: `tauri-plugin-updater` would compile an HTTP client and a TLS stack into every
binary whether the switch was on or off, turning a compile-time guarantee into a runtime
flag, and a Homebrew cask whose app updates itself has to opt out of `brew upgrade`
managing it at all.

So **the shipped application makes no network call of any kind.** `brew upgrade --cask
toolog` is the update path, and `tauri-plugin-updater` is named in the same test that
rejects `reqwest`, so restoring the exception means arguing with a failing build. See the
addendum to [ADR-0008](docs/adr/0008-local-only-zero-egress.md).

## Your controls

| Control | Where |
|---|---|
| Pause and resume capture | The menu bar |
| Never capture a project | `excluded_projects` in `prefs.json` — its transcript is never opened |
| Remove history by age or size | `toolog purge --older-than N` / `--max-size MB` |
| Remove one session, or one project | `toolog purge --session ID` / `--project PATH` |
| Redact the evidence store as well as the projection | Status → Privacy |
| Notifications on refusals and high-severity rule hits | Status → Notifications. Off by default |
| Add or retune redaction patterns | `redaction.toml`, beside the database |
| Add or retune risk rules | `rules.toml`, beside the database |
| Remove toolog and put `settings.json` back | `toolog uninstall`, or Status → Remove toolog |
| Delete everything it recorded | `toolog uninstall --delete-data`, or `brew uninstall --zap` |

**`toolog purge` deletes nothing without `--apply`.** It first prints the sessions it would
remove — named, with their project, size and last activity — because "I ran it to see what it
would do" must not be how an audit trail is lost.

**Excluding a project means never capturing it**, not hiding it. The transcript is never
opened, so nothing from it is stored and there is nothing to purge later.

## What you can check for yourself

The point of a local audit trail is that you do not have to take its word for anything.

```
toolog verify            # what the record is missing, and when nothing was watching
toolog verify --chain    # whether stored evidence has been altered since it was written
```

**Completeness.** Every stored record is reconciled across the two sources. `toolog verify`
reports how much of the *approval* layer survives, per session, and names the windows in
which nothing was watching — the periods when the OTLP lane was not running. A store
imported from history before toolog existed will honestly say most of it has no approval
record, because it does not.

**Integrity.** Every record carries a hash of itself linked to the record before it, written
in the same statement that stores it. `toolog verify --chain` recomputes the whole chain and
reports where it first stops being true, and prints a **head** — one string covering every
record before it.

Two honest limits, stated because they matter:

- Walking the chain catches any edit that leaves the rest of it alone: a changed body, a
  changed source, a deleted or reordered record. It **cannot** catch a rewrite that re-seals
  everything after the edit, because such a chain is consistent with itself. That is what the
  head is for — **keep it somewhere outside the database** and a later run reporting a
  different head for the same records gives the rewrite away.
- Deleting a record from the *middle* leaves the head untouched. So keeping the head is not a
  substitute for walking, and walking is not a substitute for keeping the head.
- A purge breaks the chain **on purpose**, and records what it removed. `verify --chain`
  reports such a break as accounted for and exits zero; a break nothing accounts for is the
  one to look at, and exits non-zero.

## Encryption at rest

**The database is not encrypted, and the honest advice is to turn on FileVault.**

SQLCipher was evaluated in Phase 7 and declined for v1. The reasoning is recorded in full as an
addendum to [ADR-0008]; the short version is that the key would have to live on this machine, so
it would defend a stolen disk or a copied backup and not the logged-in session — and macOS
already does the first, better, for the whole disk. Encrypting this file while Claude Code's own
transcripts sit in plaintext beside it, holding everything this file holds and more, would be
theatre.

It also costs something worth keeping: `sqlite3 toolog.db` works today, and that is what lets
you check every claim on this page without trusting this program.

## Uninstalling — *Phase 8*


Uninstall removes the LaunchAgent, restores `~/.claude/settings.json` from the backup
taken before toolog modified it, and asks before deleting the database — defaulting to
keeping it.

[ADR-0006]: docs/adr/0006-configure-via-settings-env-block.md
[ADR-0008]: docs/adr/0008-local-only-zero-egress.md
