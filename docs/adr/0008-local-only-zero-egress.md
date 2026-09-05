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
