# ADR-0006 — Configure via the `settings.json` `env` block, using per-signal OTEL variables

- **Status:** Accepted
- **Date:** 2026-09-04
- **Relates to:** [ADR-0005](0005-embedded-otlp-receiver.md), [ADR-0008](0008-local-only-zero-egress.md)

## Context

Claude Code's telemetry is switched on by environment variables. Something has to set them, and the
brief demands the install be dead simple.

Two placement options exist. Shell rc files (`~/.zshrc`) are the obvious route but are fragile:
editing them programmatically is error-prone, the change is not atomic, it does not apply to
already-open shells, and it misses Claude Code launched from an IDE or GUI context rather than an
interactive shell.

Claude Code's `settings.json` supports an **`env` block**, applied by Claude Code itself regardless
of how it was launched. `~/.claude/settings.json` is the user-level file in the precedence stack.
The owner's machine currently has **no `env` key and no OTEL variables set anywhere** — a clean
slate, but the tool cannot assume that of every user.

**The sharp edge is `OTEL_EXPORTER_OTLP_ENDPOINT`.** That variable is *global across signals*.
Setting it would redirect not just logs but metrics and traces to this app — silently breaking the
pipeline of any user already exporting to a corporate collector, and quietly diverting their
organization's telemetry to a local process. That is an unacceptable side effect for a tool whose
entire premise is trustworthiness.

OTEL defines per-signal overrides for exactly this reason.

## Decision

**Write a merged `env` block into `~/.claude/settings.json`, using only per-signal variables.**

```json
{ "env": {
    "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
    "OTEL_LOGS_EXPORTER": "otlp",
    "OTEL_EXPORTER_OTLP_LOGS_PROTOCOL": "http/protobuf",
    "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT": "http://127.0.0.1:47318",
    "OTEL_LOGS_EXPORT_INTERVAL": "2000",
    "OTEL_LOG_TOOL_DETAILS": "1"
} }
```

Binding rules, all enforced in code:

1. **Never set `OTEL_EXPORTER_OTLP_ENDPOINT`** or any other cross-signal variable.
2. **Merge, never overwrite.** Read the existing file, add only these keys, preserve everything else
   byte-for-byte where possible.
3. **Write atomically** (temp file plus rename) with a **timestamped backup** kept for the uninstall
   revert path.
4. **Abort with a clear message** if a non-loopback OTEL logs endpoint is already configured. Do not
   silently take over someone's existing telemetry.
5. **Never enable content capture by default.** `OTEL_LOG_USER_PROMPTS` and
   `OTEL_LOG_ASSISTANT_RESPONSES` are deliberately absent (ADR-0008).

`OTEL_LOG_TOOL_DETAILS=1` *is* set: it is what populates `tool_parameters` on decision events, which
is the audit layer this tool exists for. Its content stays local by construction (ADR-0008).

`OTEL_LOGS_EXPORT_INTERVAL` is lowered from the 5000 ms default to 2000 ms. The destination is
loopback, so the cost is negligible and the live view feels current.

This is implemented as `toolog doctor`, which reports status read-only and only mutates under
`--fix` or explicit consent in the first-run wizard.

## Consequences

**Positive**

- Installation is one file write. No shell restart, no rc editing, and it applies however Claude
  Code is launched.
- Users with existing telemetry pipelines keep them intact.
- `doctor` gives a single command that explains the state of the integration, which is most of the
  support burden for a tool like this.
- The backup makes uninstall a genuine revert rather than a best-effort guess.

**Negative**

- The app writes to a file it does not own. Mitigated by atomicity, backup, merge-only semantics and
  a tested revert — and the user consents explicitly before the first write.
- Managed/enterprise settings take precedence over the user file, so a managed OTEL policy will win.
  `doctor` must detect and explain this rather than appear broken.
- If the port changes (ADR-0005 conflict fallback), the config and the `env` block must be rewritten
  together or the two silently disagree.

## Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| Edit `~/.zshrc` or `~/.bash_profile` | Not atomic, shell-specific, misses non-interactive and GUI launches, and mangling a user's shell config is a bad failure mode. |
| Set the global `OTEL_EXPORTER_OTLP_ENDPOINT` | Hijacks metrics and traces as well as logs, breaking existing corporate pipelines and redirecting telemetry the user did not intend to redirect. |
| Print the variables and let the user paste them | Fails the "dead simple installation" constraint and guarantees inconsistent setups to support. |
| Launch Claude Code through a wrapper that injects the env | Intercepts the user's own tooling and breaks every other launch path. |
