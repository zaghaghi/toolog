//! Turning stored OTLP evidence into rows.
//!
//! Implements [`toolog_core::project::Projector`] over `raw_event` bodies, so
//! live ingest and re-projection take exactly the same path — and a decoding fix
//! can be replayed over history without the events being re-sent.

use serde_json::Value;
use toolog_core::model::RawEvent;
use toolog_core::project::{self, Projector};
use toolog_core::{Connection, Result};

use crate::events::{self, Event};

/// What a projection run saw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OtlpStats {
    pub records: usize,
    pub tool_results: usize,
    pub tool_decisions: usize,
    /// Refusals — the events transcripts can never contain.
    pub rejections: usize,
    pub api_requests: usize,
    pub prompts: usize,
    /// Recognised as events, but not ones this build projects.
    pub other: usize,
    pub unparsable: usize,
}

/// Projects OTLP log records into `toolog-core` tables.
#[derive(Debug, Default)]
pub struct OtlpProjector {
    stats: OtlpStats,
}

impl OtlpProjector {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn stats(&self) -> &OtlpStats {
        &self.stats
    }

    /// Project one stored record body.
    pub fn project_body(&mut self, conn: &Connection, body: &str) -> Result<()> {
        let Ok(record) =
            serde_json::from_str::<opentelemetry_proto::tonic::logs::v1::LogRecord>(body)
        else {
            self.stats.unparsable += 1;
            return Ok(());
        };

        self.stats.records += 1;
        let common = events::common(&record);

        // Any event naming a session establishes it, so a rejection in a session
        // whose transcript has not been read yet still has somewhere to hang.
        if common.session_id.is_some() {
            project::upsert_session(conn, &events::session(&common))?;
        }

        match events::classify(&record, &common) {
            Event::ToolResult { tool_use_id, facts } => {
                self.stats.tool_results += 1;
                project::upsert_otel(conn, &tool_use_id, &facts)?;
            }
            Event::ToolDecision { tool_use_id, facts } => {
                self.stats.tool_decisions += 1;
                if facts.decision.as_deref() == Some("reject") {
                    self.stats.rejections += 1;
                }
                project::upsert_otel(conn, &tool_use_id, &facts)?;
            }
            Event::ApiRequest(r) => {
                self.stats.api_requests += 1;
                project::upsert_api_request(conn, &r)?;
            }
            Event::Prompt(p) => {
                self.stats.prompts += 1;
                project::upsert_prompt(conn, &p)?;
            }
            Event::PermissionModeChanged(c) => project::insert_permission_mode_change(conn, &c)?,
            Event::SessionSeen(s) => project::upsert_session(conn, &s)?,
            Event::Other => self.stats.other += 1,
        }
        Ok(())
    }
}

impl Projector for OtlpProjector {
    fn project(&mut self, conn: &Connection, event: &RawEvent) -> Result<()> {
        // Only this lane's records; a mixed re-projection sees both.
        if event.lane != toolog_core::model::Lane::Otlp.as_str() {
            return Ok(());
        }
        self.project_body(conn, &event.body)
    }
}

/// Canonical JSON for one log record, as stored in `raw_event`.
///
/// A protobuf request is re-encoded to OTLP/JSON rather than stored as bytes.
/// It is the same message losslessly re-encoded, not a summary, and it keeps
/// `raw_event` uniformly JSON — which the re-projection path and the Phase 7
/// integrity chain both rely on.
#[must_use]
pub fn record_json(record: &opentelemetry_proto::tonic::logs::v1::LogRecord) -> String {
    serde_json::to_string(record).unwrap_or_else(|_| Value::Null.to_string())
}
