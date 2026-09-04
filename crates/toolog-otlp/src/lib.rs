//! The OTLP lane: the *decision* half of the audit trail ([ADR-0002]).
//!
//! Carries what transcripts never record — who approved each call and how, how
//! long it took, what it cost, and the calls that were **rejected**. A denied
//! tool call leaves no transcript trace whatsoever, so this lane is the only way
//! a refusal is ever observed.
//!
//! The receiver is embedded rather than delegated to `otelcol` ([ADR-0005]): the
//! brief asks for a one-artifact install, and Claude Code is the only client,
//! speaking OTLP over loopback HTTP.
//!
//! Implementation lands in Phase 3.
//!
//! [ADR-0002]: ../../../docs/adr/0002-dual-ingestion-transcripts-and-otel.md
//! [ADR-0005]: ../../../docs/adr/0005-embedded-otlp-receiver.md
