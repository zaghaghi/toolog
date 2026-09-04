//! Decoding an OTLP export request.
//!
//! Both wire formats decode into the same generated types ([ADR-0005]):
//! `http/protobuf` through prost, `http/json` through the same structs' serde
//! implementation, chosen by `Content-Type`.
//!
//! [ADR-0005]: ../../../docs/adr/0005-embedded-otlp-receiver.md

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use prost::Message;

/// Which encoding a request body is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Protobuf,
    Json,
}

impl Encoding {
    /// Pick an encoding from a `Content-Type` header.
    ///
    /// Returns `None` for anything else, which the caller answers with 415
    /// rather than guessing — a misconfigured exporter should be told, not
    /// silently half-understood.
    #[must_use]
    pub fn from_content_type(value: &str) -> Option<Self> {
        let mime = value
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match mime.as_str() {
            "application/x-protobuf" | "application/protobuf" => Some(Self::Protobuf),
            "application/json" => Some(Self::Json),
            _ => None,
        }
    }
}

/// Why a body could not be decoded.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("unsupported content type: {0}")]
    UnsupportedContentType(String),
    #[error("malformed protobuf: {0}")]
    Protobuf(#[from] prost::DecodeError),
    #[error("malformed json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Decode an export request body.
pub fn logs(encoding: Encoding, body: &[u8]) -> Result<ExportLogsServiceRequest, DecodeError> {
    Ok(match encoding {
        Encoding::Protobuf => ExportLogsServiceRequest::decode(body)?,
        Encoding::Json => serde_json::from_slice(body)?,
    })
}

/// Flatten an export request into its log records, in arrival order.
///
/// The resource/scope nesting carries nothing this receiver needs: Claude Code
/// puts every attribute the audit trail uses on the record itself.
#[must_use]
pub fn records(request: ExportLogsServiceRequest) -> Vec<LogRecord> {
    request
        .resource_logs
        .into_iter()
        .flat_map(|rl| rl.scope_logs)
        .flat_map(|sl| sl.log_records)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    #[test]
    fn recognises_both_content_types_and_rejects_others() {
        assert_eq!(
            Encoding::from_content_type("application/x-protobuf"),
            Some(Encoding::Protobuf)
        );
        assert_eq!(
            Encoding::from_content_type("application/json; charset=utf-8"),
            Some(Encoding::Json)
        );
        assert_eq!(
            Encoding::from_content_type("  APPLICATION/JSON "),
            Some(Encoding::Json)
        );
        assert_eq!(Encoding::from_content_type("text/plain"), None);
        assert_eq!(Encoding::from_content_type(""), None);
    }

    /// The core claim of ADR-0005: one set of types, both wire formats, same
    /// result.
    #[test]
    fn both_encodings_decode_to_identical_records() {
        let request = testing::request(vec![testing::tool_result("toolu_1", true, 42)]);

        let from_json =
            records(logs(Encoding::Json, &serde_json::to_vec(&request).expect("json")).expect("d"));
        let from_proto = records(logs(Encoding::Protobuf, &request.encode_to_vec()).expect("d"));

        assert_eq!(from_json.len(), 1);
        assert_eq!(format!("{from_json:?}"), format!("{from_proto:?}"));
    }

    #[test]
    fn malformed_bodies_error_rather_than_panic() {
        assert!(logs(Encoding::Json, b"not json").is_err());
        assert!(logs(Encoding::Json, b"").is_err());
        // Field 1 declared as length-delimited with a length past the end.
        assert!(logs(Encoding::Protobuf, &[0x0a, 0xff, 0x01]).is_err());
        // A truncated but otherwise valid protobuf body.
        let full = testing::request(vec![testing::tool_result("toolu_1", true, 1)]).encode_to_vec();
        assert!(logs(Encoding::Protobuf, &full[..full.len() / 2]).is_err());
    }

    #[test]
    fn an_empty_export_is_valid_and_yields_nothing() {
        let empty = ExportLogsServiceRequest::default();
        assert!(records(logs(Encoding::Protobuf, &empty.encode_to_vec()).expect("d")).is_empty());
    }

    #[test]
    fn records_from_several_scopes_are_flattened_in_order() {
        let request = testing::request(vec![
            testing::tool_result("toolu_a", true, 1),
            testing::tool_result("toolu_b", false, 2),
        ]);
        let flat = records(request);
        assert_eq!(flat.len(), 2);
        // The `claude_code.` qualifier is stripped: a live session showed the
        // attribute carrying the bare name while the body carries it qualified.
        assert_eq!(crate::events::common(&flat[0]).event_name, "tool_result");
    }
}
