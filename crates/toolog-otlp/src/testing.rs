//! Builders for OTLP payloads, used by this crate's tests and by
//! `toolog-ingest`'s cross-lane tests.
//!
//! Shaped after real `claude_code.*` events: the attribute names and value types
//! are the ones Claude Code emits.

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, ArrayValue, KeyValue, any_value::Value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};

/// A string attribute.
#[must_use]
pub fn s(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(Value::StringValue(value.into())),
        }),
        ..KeyValue::default()
    }
}

/// An integer attribute.
#[must_use]
pub fn i(key: &str, value: i64) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(Value::IntValue(value)),
        }),
        ..KeyValue::default()
    }
}

/// A boolean attribute.
#[must_use]
pub fn b(key: &str, value: bool) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(Value::BoolValue(value)),
        }),
        ..KeyValue::default()
    }
}

/// A string-array attribute.
#[must_use]
pub fn arr(key: &str, values: &[&str]) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(Value::ArrayValue(ArrayValue {
                values: values
                    .iter()
                    .map(|v| AnyValue {
                        value: Some(Value::StringValue((*v).into())),
                    })
                    .collect(),
            })),
        }),
        ..KeyValue::default()
    }
}

/// A log record with the given attributes, timestamped in milliseconds.
///
/// `event_name` is the unqualified name. The record carries it exactly as a live
/// Claude Code session does: **bare** in the `event.name` attribute, and
/// **qualified** with `claude_code.` in the body.
#[must_use]
pub fn record(event_name: &str, ts_ms: u64, mut attributes: Vec<KeyValue>) -> LogRecord {
    use opentelemetry_proto::tonic::common::v1::AnyValue as Body;

    attributes.insert(0, s("event.name", event_name));
    attributes.push(s("event.timestamp", "2026-09-04T19:57:47.987Z"));
    LogRecord {
        time_unix_nano: ts_ms * 1_000_000,
        observed_time_unix_nano: ts_ms * 1_000_000,
        severity_number: 9,
        severity_text: "INFO".into(),
        body: Some(Body {
            value: Some(Value::StringValue(format!("claude_code.{event_name}"))),
        }),
        attributes,
        ..LogRecord::default()
    }
}

/// A `claude_code.tool_result` for an accepted Bash call.
#[must_use]
pub fn tool_result(tool_use_id: &str, success: bool, duration_ms: i64) -> LogRecord {
    record(
        "claude_code.tool_result",
        1_700_000_000_000,
        vec![
            s("tool_use_id", tool_use_id),
            s("tool_name", "Bash"),
            b("success", success),
            i("duration_ms", duration_ms),
            s("decision_source", "config"),
            s("session.id", "s-otel"),
            s("prompt.id", "p-1"),
            s("app.version", "2.1.260"),
            arr("workspace.host_paths", &["/work/app"]),
            i("event.sequence", 1),
        ],
    )
}

/// A `tool_decision` carrying the attempted arguments, as a live session sends
/// them when `OTEL_LOG_TOOL_DETAILS=1`.
#[must_use]
pub fn tool_decision_with_input(
    tool_use_id: &str,
    decision: &str,
    source: &str,
    tool_name: &str,
    tool_input: &str,
) -> LogRecord {
    record(
        "tool_decision",
        1_700_000_000_000,
        vec![
            s("tool_use_id", tool_use_id),
            s("tool_name", tool_name),
            s("decision", decision),
            s("source", source),
            s("tool_source", "builtin"),
            s("tool_input", tool_input),
            i(
                "tool_input_size_bytes",
                i64::try_from(tool_input.len()).unwrap_or(0),
            ),
            s("session.id", "s-otel"),
        ],
    )
}

/// A `tool_decision`. `decision` is `accept` or `reject`.
#[must_use]
pub fn tool_decision(tool_use_id: &str, decision: &str, source: &str) -> LogRecord {
    record(
        "claude_code.tool_decision",
        1_700_000_000_000,
        vec![
            s("tool_use_id", tool_use_id),
            s("tool_name", "Bash"),
            s("decision", decision),
            s("source", source),
            s("tool_source", "builtin"),
            s("session.id", "s-otel"),
            s("prompt.id", "p-1"),
        ],
    )
}

/// A `claude_code.api_request` with cost and token counts.
#[must_use]
pub fn api_request(request_id: &str, cost_micros: i64) -> LogRecord {
    record(
        "claude_code.api_request",
        1_700_000_000_000,
        vec![
            s("request_id", request_id),
            s("model", "claude-opus-5"),
            i("cost_usd_micros", cost_micros),
            i("input_tokens", 100),
            i("output_tokens", 200),
            i("cache_read_tokens", 50),
            i("duration_ms", 1234),
            s("speed", "normal"),
            s("effort", "high"),
            s("query_source", "repl_main_thread"),
            s("session.id", "s-otel"),
        ],
    )
}

/// Wrap records in an export request, as an exporter would send them.
#[must_use]
pub fn request(records: Vec<LogRecord>) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: None,
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: records,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}
