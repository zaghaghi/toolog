# ADR-0008 — Local-only: loopback bind, zero egress, opt-in content capture

- **Status:** Accepted
- **Date:** 2026-09-04
- **Relates to:** [ADR-0005](0005-embedded-otlp-receiver.md), [ADR-0006](0006-configure-via-settings-env-block.md)

## Context

The brief's first constraint is absolute: *"Local only, it MUST work locally without sending users
data over the network."*

The data involved is unusually sensitive even by developer-tool standards. It includes every shell
command run in every repository, full file contents from reads and writes, file paths that reveal
project and client names, and — if content logging were enabled — prompts and model responses. It is
a near-complete record of a developer's work.

An audit tool asks for a lot of trust. It has to be structurally worth it, not merely well-intentioned.
A privacy claim resting on "we don't call any APIs" is a claim about present code, and present code
changes. It needs to be a property the build enforces.

## Decision

**No user data leaves the machine, and the constraint is tested rather than promised.**

1. **Loopback only.** The OTLP receiver binds `127.0.0.1` explicitly — never `0.0.0.0`, never an
   interface address. Not reachable from the local network.
2. **No egress.** No analytics, no crash reporting, no telemetry about this tool, no remote
   configuration, no license check.
3. **Content capture is opt-in and off by default.** `OTEL_LOG_USER_PROMPTS` and
   `OTEL_LOG_ASSISTANT_RESPONSES` are deliberately absent from the `env` block written in ADR-0006.
   Tool inputs and results *are* captured, because they are the audit trail's substance — and they
   never leave the disk they are written to.
4. **Enforced in CI.** A test asserts that **no non-loopback socket is opened** during a full ingest
   plus UI-query run. Per this ADR the guarantee is a build failure when broken, not a convention.
5. **Local storage is plainly documented.** `PRIVACY.md` states exactly what is stored, where, and
   what never leaves. Retention caps, per-project exclusion, pause-capture, delete-a-session and
   secret redaction all land in Phase 7.
6. **One narrow exception, named explicitly.** The Phase 8 update check contacts GitHub Releases for
   a version manifest. It is **off by default, opt-in at first run, sends no user data, and is named
   in the README and `PRIVACY.md`** as the single network call the application can make. An
   undisclosed exception would be worse than no update mechanism.

## Consequences

**Positive**

- The core promise is verifiable by anyone: read the CI test, or watch the process's sockets.
- Loopback binding means an untrusted network cannot reach the receiver, so no authentication layer
  is needed on the OTLP endpoint.
- No accounts, no keys, no server, nothing to breach centrally.

**Negative**

- No cross-machine aggregation, no team dashboards, no cloud backup. Out of scope by design; a
  future team edition would need a separate, explicitly-consented architecture and a new ADR.
- No crash telemetry, so field failures depend on user reports.
- Update checking requires an explicit opt-in prompt, which is friction. Correct friction.

**Neutral**

- Any user on the machine with filesystem access can read the database. Encryption at rest is
  evaluated in Phase 7 (SQLCipher), with the honest caveat that a local key mainly defends the
  stolen-disk and backup cases.

## Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| Anonymous usage analytics | Directly contradicts the brief. Anonymization of shell commands and file paths is not achievable in any case. |
| Opt-in cloud sync | Out of scope, and would make the local-only guarantee conditional rather than structural. |
| Crash reporting via Sentry or similar | Stack traces from this app can carry file paths and command fragments. Not worth the exception. |
| Trust the code review, skip the CI egress test | The guarantee then depends on nobody making a mistake later. The test is cheap and makes it durable. |
| Bind `0.0.0.0` for convenience | Exposes the receiver to the local network, allowing anyone on it to inject fabricated audit events. |

## Addendum — encryption at rest, evaluated and declined for v1 (task 7.9)

**Decision: no SQLCipher for v1.** The database stays a plain SQLite file. Revisit if a
"shared or managed machine" story appears, where the threat model changes.

The evaluation, since the decision only means something with the reasoning attached.

**What it would defend.** SQLCipher encrypts pages with AES-256, so a database file taken
*away* from the running machine is unreadable: a stolen laptop with FileVault off, a backup
copied to a drive or a cloud folder, a disk sent for repair. Those are real cases.

**What it would not.** The key has to live on the same machine, because nothing else is
allowed to (this ADR). Wherever it lives — a file beside the database, the login keychain,
a passphrase the user types — anything running as that user while they are logged in can
reach it, and the tool itself must reach it on every launch to work at all. So it defends
the disk, not the session; and the session is where the risk actually is, because the thing
being protected is a record of what an agent did *on that machine as that user*.

**What it would cost.**

| | |
|---|---|
| Dependency | `libsqlite3-sys` with the `bundled-sqlcipher` feature, replacing the plain bundled build. A second C library in the build, on both platforms. |
| Key handling | A new decision with no good default: a keychain entry the user never sees, or a passphrase they must type at every launch, for a menu-bar app that is meant to start at login and be forgotten. |
| Recoverability | A lost key is a lost database. The evidence store exists so the projection can be rebuilt; encryption adds a way to lose both at once. |
| Operability | `sqlite3 toolog.db` stops working. PRIVACY.md's "readable with any sqlite3 binary" is a feature, not an accident — it is what lets a user check the claims here without trusting this program. |
| Performance | Measured elsewhere at roughly 5–15% on read-heavy work. Not decisive, and not free. |

**Why the balance falls where it does.** macOS ships FileVault, and it is on by default on
current hardware. It encrypts the whole disk with a key held in the Secure Enclave, which is
strictly better than what this application could arrange for one file — it covers the
transcripts in `~/.claude/projects` too, and those hold everything this database holds and
more. Encrypting our copy while Claude Code's originals sit in plaintext beside it would be
security theatre: the same content, one copy locked, one not.

**So the honest advice, which PRIVACY.md now gives, is to turn on FileVault.** That is a
better answer than SQLCipher for the case SQLCipher would cover, and it is one line to say.

**What would change the decision.** A shared or managed machine, where "any user on this
machine with filesystem access" stops being the same person as the user being recorded. That
is a different product with a different threat model, and it would need its own ADR — along
with the key-handling decision this one declines to make.

## Addendum — the update check, evaluated and declined (task 8.5)

**Decision: no update mechanism of any kind in v1.0.** Point 6 above reserved one narrow
exception. Phase 8 declined to take it, so **there is no exception**: the shipped application
makes no network call at all. `brew upgrade --cask toolog` is the update path.

Point 6 is superseded by this addendum. It is left in place above rather than edited out,
because a decision record that quietly rewrites what it decided is not a record.

**What was reserved, and why it looked right.** A tool that captures a security-relevant
record should be patchable, and a user who downloads a `.dmg` directly has no way to learn a
new version exists. `tauri-plugin-updater` with signed manifests, opt-in at first run, off by
default, was the plan.

**What changed the answer.** Three things, none of them visible from Phase 0.

| | |
|---|---|
| The guarantee had become structural | Phase 7.7 built `no_manifest_in_the_workspace_asks_for_an_outbound_client`, and with it a property stronger than the ADR asked for: no HTTP client is *compiled into* this binary. The updater's `reqwest` and TLS stack would be linked whether the switch was on or off, so "off by default" would demote a compile-time fact to a runtime flag. That is a real loss, and it is exactly the loss the rejected alternative "trust the code review, skip the CI egress test" describes. |
| Homebrew is the distribution channel | A cask whose application updates itself has to declare `auto_updates true`, after which `brew upgrade` stops managing it. An in-app updater does not add to `brew`; it takes over from it, and leaves the version Homebrew believes is installed wrong. |
| The exception was not free to explain | ADR-0008's own reasoning is that a privacy claim has to be checkable rather than trusted. "No network calls, except one, which is off unless you turned it on" is a materially weaker sentence than "no network calls", and the README's opening claim is the thing this project is actually selling. |

**What it costs.** A direct-download user is not told when a new version exists. That is the
whole cost, and it is borne by the smaller half of the audience: `brew upgrade` covers the
other half properly, and `livecheck` in the cask means Homebrew notices new releases without
this application asking anything.

**What replaces it: nothing in the binary.** No version ping, no "check for updates" menu
item, not even one that opens a browser — a menu item that exists only to send you to a page
is a feature whose absence costs a sentence in the README, which is where it now lives.
`toolog --version` says what you have; the releases page says what exists.

**How the decision is enforced.** `tauri-plugin-updater` and `self_update` are named in
`OUTBOUND_CLIENTS` in `crates/toolog-cli/tests/egress.rs`, so adding either fails the build
with a message pointing back here. The decision is a red test, not a convention.

**What would change it.** Distribution to people who cannot use Homebrew and will not
re-download — a managed fleet, or a security fix urgent enough that "the next time you run
`brew upgrade`" is not fast enough. Either would need this addendum revisited and the README's
front page rewritten in the same commit.
