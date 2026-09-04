# ADR-0005 — Embed the OTLP receiver; never require the OpenTelemetry Collector

- **Status:** Accepted
- **Date:** 2026-09-04
- **Relates to:** [ADR-0002](0002-dual-ingestion-transcripts-and-otel.md), [ADR-0006](0006-configure-via-settings-env-block.md), [ADR-0007](0007-single-resident-process.md)

## Context

The brief's architecture names a "Collector" between Claude Code and the database. In OpenTelemetry
usage that word normally means `otelcol` — a separate binary with its own YAML configuration,
lifecycle and update cadence.

That reading collides head-on with the brief's second constraint: *"Dead simple installation, one
binary file to install."* Requiring users to install, configure and keep `otelcol` running would
make the install a multi-step, multi-artifact process and hand users a second thing that can break.

The actual surface needed is small. Claude Code is the only client, it speaks OTLP over HTTP to a
loopback address, and only the logs signal carries the events this tool wants. There is no fan-out,
no sampling, no tail-based processing, no multi-tenant routing — none of what `otelcol` exists for.

## Decision

**Implement the OTLP receiver inside the application.**

- An `axum` server bound to `127.0.0.1:47318` (see ADR-0008 for the loopback requirement).
- `POST /v1/logs` — the signal that matters.
- `POST /v1/metrics` — accepted and dropped initially, so a user who enables metrics gets a clean
  204 rather than connection errors in their Claude Code debug log.
- `GET /healthz` — used by `toolog doctor` and the tray status indicator.
- Accept both `http/protobuf` (via `opentelemetry-proto`/prost, the installed default) and
  `http/json`, branching on `Content-Type`.

A distinctive default port, **47318**, rather than the conventional 4318. Since ADR-0006 has us
write the endpoint into Claude Code's configuration, there is no benefit to squatting the standard
port, and real cost if the user already runs a collector there. All of 4317, 4318 and 47318 were
free on the owner's machine; the port is configurable and probed at startup regardless.

## Consequences

**Positive**

- The install stays one artifact, satisfying the brief directly.
- Nothing to keep running, update or configure besides this app.
- Events reach the database in-process — no second network hop, no intermediate queue, no
  serialization round-trip.
- Failures are diagnosable in one place, and `toolog doctor` can check the whole path end to end.

**Negative**

- Two OTLP encodings to decode and keep tested (mitigated by golden payload fixtures in Phase 3).
- The endpoint only exists while the app runs. ADR-0007's LaunchAgent keeps it resident; a lapse
  costs only the decision/cost layer, since transcripts are read from disk afterwards.
- If Claude Code changes OTLP protocol versions, this must follow. ADR-0004's raw-first persistence
  means a decoding gap loses nothing that cannot be re-projected.

**Neutral**

- Users with an existing corporate collector are not blocked; ADR-0006's per-signal configuration
  leaves their metrics pipeline untouched.

## Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| Require `otelcol` alongside the app | Directly violates the one-artifact install. Adds a second lifecycle, a YAML config, and a second thing to debug — for capability this app does not use. |
| Bundle `otelcol` inside the `.app` | Keeps the install single-artifact but adds a child process, IPC, and tens of megabytes, still for unused capability. |
| gRPC (OTLP's other transport) | Pulls in tonic and a TLS surface for zero gain over loopback HTTP. `http/protobuf` is fully supported by Claude Code's exporter. |
| Use `OTEL_METRICS_EXPORTER=prometheus` and scrape | Metrics only; the tool-level events this app is built on are logs, not metrics. |
