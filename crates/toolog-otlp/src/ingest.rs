//! Storing a decoded batch, then projecting it.
//!
//! The order is the point ([ADR-0004]): every record reaches `raw_event` before
//! anything tries to understand it. An event type added in a future Claude Code
//! release is kept, and a later re-projection picks it up.
//!
//! [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md

use opentelemetry_proto::tonic::logs::v1::LogRecord;
use toolog_core::model::{Lane, NewRawEvent};
use toolog_core::{Connection, Result, raw};

use crate::projector::{OtlpProjector, record_json};

/// What one batch did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestStats {
    pub received: usize,
    /// Records not already held by content hash. A retried export dedups here.
    pub stored: usize,
    pub projected: crate::projector::OtlpStats,
}

/// Store and project one batch of records.
///
/// `source_ref` identifies the batch in `raw_event`, for tracing a row back to
/// the export that carried it.
pub fn ingest_records(
    conn: &Connection,
    source_ref: &str,
    records: &[LogRecord],
) -> Result<IngestStats> {
    let mut stats = IngestStats {
        received: records.len(),
        ..IngestStats::default()
    };
    let bodies: Vec<String> = records.iter().map(record_json).collect();

    let tx = conn.unchecked_transaction()?;
    let mut projector = OtlpProjector::new();

    for body in &bodies {
        let event = NewRawEvent {
            lane: Lane::Otlp,
            source_ref,
            source_offset: None,
            body,
        };
        // A duplicate is a retried export, already stored and already projected.
        if raw::insert(&tx, &event)?.is_new() {
            stats.stored += 1;
            projector.project_body(&tx, body)?;
        }
    }
    tx.commit()?;

    stats.projected = projector.stats().clone();
    Ok(stats)
}
