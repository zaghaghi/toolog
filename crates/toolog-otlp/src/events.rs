//! Mapping Claude Code's OTLP events onto `toolog` rows.
//!
//! This is the decision lane of [ADR-0002] — everything transcripts cannot
//! record. Most of it is convenience (durations, cost, tokens), but one part is
//! irreplaceable: `claude_code.tool_decision` is the only place **who refused a
//! call, and under which rule**, is ever stated. The transcript keeps a record
//! of the refusal, but only as prose inside a result string.
//!
//! Unrecognised events are returned as [`Event::Other`] rather than dropped. The
//! record is already in `raw_event` by the time this runs ([ADR-0004]), so a new
//! event type costs nothing but a later re-projection.
//!
//! [ADR-0002]: ../../../docs/adr/0002-dual-ingestion-transcripts-and-otel.md
//! [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md

use opentelemetry_proto::tonic::logs::v1::LogRecord;
use serde_json::Value;
use toolog_core::model::{ApiRequest, OtelFacts, PermissionModeChange, Prompt, Session};
use toolog_core::normalize;

use crate::attrs::Attrs;

/// Nanoseconds per millisecond.
const NANOS_PER_MS: u64 = 1_000_000;

/// Parse an ISO-8601 instant to milliseconds since the epoch.
fn parse_iso8601_ms(s: &str) -> Option<i64> {
    s.parse::<jiff::Timestamp>()
        .ok()
        .map(jiff::Timestamp::as_millisecond)
}

/// Facts every event carries, regardless of type.
#[derive(Debug, Clone, Default)]
pub struct Common {
    pub event_name: String,
    pub session_id: Option<String>,
    pub prompt_id: Option<String>,
    pub message_uuid: Option<String>,
    pub app_version: Option<String>,
    pub workspace_path: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub timestamp_ms: Option<i64>,
    pub sequence: Option<i64>,
}

/// One decoded Claude Code event.
#[derive(Debug, Clone)]
pub enum Event {
    /// A tool ran. Carries duration and outcome.
    ToolResult {
        tool_use_id: String,
        facts: Box<OtelFacts>,
    },
    /// A tool call was allowed or refused, and by what.
    ///
    /// The only record of a refusal that exists anywhere.
    ToolDecision {
        tool_use_id: String,
        facts: Box<OtelFacts>,
    },
    ApiRequest(Box<ApiRequest>),
    Prompt(Box<Prompt>),
    PermissionModeChanged(Box<PermissionModeChange>),
    /// A session began, or an event named one this build does not otherwise map.
    SessionSeen(Box<Session>),
    /// Recognised as an event, but not one this build projects.
    Other,
}

/// Extract the facts common to every event.
#[must_use]
pub fn common(record: &LogRecord) -> Common {
    let a = Attrs(&record.attributes);

    // A live session showed the name in three places and the forms differ:
    // the `event.name` attribute carries the bare name (`tool_result`) while the
    // record body carries it qualified (`claude_code.tool_result`). The
    // OTLP `event_name` field was empty. Take whichever is present and
    // normalize the prefix away.
    let event_name = a
        .string("event.name")
        .or_else(|| non_empty(&record.event_name))
        .or_else(|| body_string(record))
        .map(|n| strip_prefix(&n))
        .unwrap_or_default();

    let timestamp_ms = if record.time_unix_nano > 0 {
        i64::try_from(record.time_unix_nano / NANOS_PER_MS).ok()
    } else if record.observed_time_unix_nano > 0 {
        i64::try_from(record.observed_time_unix_nano / NANOS_PER_MS).ok()
    } else {
        // Claude Code also sends `event.timestamp` as ISO-8601, which survives
        // an exporter that leaves the record's own timestamps unset.
        a.str("event.timestamp").and_then(parse_iso8601_ms)
    };

    Common {
        event_name,
        session_id: a.string("session.id"),
        prompt_id: a.string("prompt.id"),
        message_uuid: a.string("message.uuid"),
        app_version: a.string("app.version"),
        workspace_path: a.first_of_array("workspace.host_paths"),
        timestamp_ms,
        sequence: a.int("event.sequence"),
    }
}

/// Map one record to an event.
#[must_use]
pub fn classify(record: &LogRecord, common: &Common) -> Event {
    let a = Attrs(&record.attributes);

    // Names here are unqualified; `common` has already stripped any
    // `claude_code.` prefix.
    match common.event_name.as_str() {
        "tool_result" => tool_result(&a, common),
        "tool_decision" => tool_decision(&a, common),
        "api_request" | "api_error" => api_request(&a, common),
        "user_prompt" => prompt(&a, common),
        "permission_mode_changed" => permission_mode(&a, common),
        "session.count" | "session_start" => Event::SessionSeen(Box::new(session(common))),
        _ => Event::Other,
    }
}

fn tool_result(a: &Attrs<'_>, c: &Common) -> Event {
    let Some(tool_use_id) = a.string("tool_use_id") else {
        return Event::Other;
    };
    let (kind, server, tool) = tool_identity(a);

    Event::ToolResult {
        tool_use_id,
        facts: Box::new(OtelFacts {
            session_id: c.session_id.clone(),
            prompt_id: c.prompt_id.clone(),
            message_uuid: c.message_uuid.clone(),
            tool_name: a.string("tool_name"),
            tool_kind: kind,
            mcp_server: server,
            mcp_tool: tool,
            called_at: c.timestamp_ms,
            duration_ms: a.int("duration_ms"),
            error_type: a.string("error_type"),
            // A tool_result is only emitted for calls that ran, so its presence
            // is itself an acceptance.
            decision: Some("accept".into()),
            decision_source: a.string("decision_source"),
            success: a.bool("success"),
            ..attempted_input(a)
        }),
    }
}

fn tool_decision(a: &Attrs<'_>, c: &Common) -> Event {
    let Some(tool_use_id) = a.string("tool_use_id") else {
        return Event::Other;
    };
    let (kind, server, tool) = tool_identity(a);
    let decision = a.string("decision");

    Event::ToolDecision {
        tool_use_id,
        facts: Box::new(OtelFacts {
            session_id: c.session_id.clone(),
            prompt_id: c.prompt_id.clone(),
            message_uuid: c.message_uuid.clone(),
            tool_name: a.string("tool_name"),
            tool_kind: kind,
            mcp_server: server,
            mcp_tool: tool,
            called_at: c.timestamp_ms,
            duration_ms: None,
            error_type: None,
            // `source` says *who* decided: config, hook, user_permanent,
            // user_temporary, user_abort or user_reject.
            decision_source: a.string("source").or_else(|| a.string("decision_source")),
            // A refused call never ran, so it has no success to report.
            success: (decision.as_deref() == Some("reject")).then_some(false),
            decision,
            ..attempted_input(a)
        }),
    }
}

/// Tool kind and MCP identity.
///
/// `tool_source` names the provider; the MCP server and tool names live inside
/// the JSON `tool_parameters` attribute rather than as attributes of their own.
fn tool_identity(a: &Attrs<'_>) -> (Option<String>, Option<String>, Option<String>) {
    let kind = match a.str("tool_source") {
        Some("mcp" | "sdk_host_builtin_mcp") => Some("mcp".to_string()),
        Some("builtin") => Some("builtin".to_string()),
        _ => None,
    };

    let params = a.json("tool_parameters");
    let get = |key: &str| {
        params
            .as_ref()
            .and_then(|p| p.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let server = get("mcp_server_name").or_else(|| a.string("mcp_server_scope"));
    let tool = get("mcp_tool_name");

    // A named MCP server settles the kind even when tool_source was absent.
    let kind = kind.or_else(|| server.as_ref().map(|_| "mcp".to_string()));
    (kind, server, tool)
}

/// What the call was *asked* to do.
///
/// Two attributes can carry it, and which one appears matters:
///
/// - **`tool_input`** — the full arguments, but a live session showed it on
///   `tool_result` only.
/// - **`tool_parameters`** — tool-specific fields, documented for both events.
///
/// A **rejected** call never runs, so it emits a decision and no result. That
/// makes `tool_parameters` the only place a refusal's target can come from, and
/// without it the audit trail would record that *a Bash call* was denied while
/// saying nothing about what it would have done.
///
/// Stored existing-wins: both forms are truncated at 512 characters per value,
/// so neither may displace a transcript's full copy.
fn attempted_input(a: &Attrs<'_>) -> OtelFacts {
    let tool = a.str("tool_name").unwrap_or_default();

    if let Some(raw) = a.str("tool_input") {
        let parsed: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
        let facts = normalize::input(tool, &parsed);
        return OtelFacts {
            attempted_input_json: Some(raw.to_string()),
            attempted_input_summary: facts.summary,
            attempted_target_path: facts.target,
            ..OtelFacts::default()
        };
    }

    let Some(params) = a.json("tool_parameters") else {
        return OtelFacts::default();
    };

    // tool_parameters uses its own key names, so translate the ones that carry a
    // target into the shapes the normalizers already understand.
    let summary = [
        "full_command",
        "bash_command",
        "skill_name",
        "subagent_type",
        "description",
    ]
    .iter()
    .find_map(|k| params.get(*k).and_then(Value::as_str))
    .map(str::to_string)
    .or_else(|| normalize::generic(&params).summary);

    OtelFacts {
        attempted_input_json: Some(params.to_string()),
        attempted_input_summary: summary,
        attempted_target_path: normalize::generic(&params).target,
        ..OtelFacts::default()
    }
}

fn api_request(a: &Attrs<'_>, c: &Common) -> Event {
    // Errors and timeouts may have no server request id; the client-generated
    // one is always there, so cost and failures are never lost.
    let Some(request_id) = a
        .string("request_id")
        .or_else(|| a.string("client_request_id"))
        .or_else(|| c.message_uuid.clone())
    else {
        return Event::Other;
    };

    #[allow(clippy::cast_possible_truncation)]
    let cost_micros = a.int("cost_usd_micros").or_else(|| {
        a.float("cost_usd")
            .map(|c| (c * 1_000_000.0).round() as i64)
    });

    Event::ApiRequest(Box::new(ApiRequest {
        request_id,
        session_id: c.session_id.clone(),
        prompt_id: c.prompt_id.clone(),
        model: a.string("model"),
        cost_usd_micros: cost_micros,
        input_tokens: a.int("input_tokens"),
        output_tokens: a.int("output_tokens"),
        cache_read_tokens: a.int("cache_read_tokens"),
        cache_creation_tokens: a.int("cache_creation_tokens"),
        duration_ms: a.int("duration_ms"),
        speed: a.string("speed"),
        effort: a.string("effort"),
        query_source: a.string("query_source"),
        agent_name: a.string("agent.name"),
        ts: c.timestamp_ms,
    }))
}

/// Prompt metadata only.
///
/// The `prompt` attribute is deliberately never read. [ADR-0008] does not enable
/// `OTEL_LOG_USER_PROMPTS`, and `prompt` has no column to be stored in even if a
/// user enabled it by hand.
///
/// [ADR-0008]: ../../../docs/adr/0008-local-only-zero-egress.md
fn prompt(a: &Attrs<'_>, c: &Common) -> Event {
    let Some(prompt_id) = c.prompt_id.clone().or_else(|| c.message_uuid.clone()) else {
        return Event::Other;
    };

    Event::Prompt(Box::new(Prompt {
        prompt_id,
        session_id: c.session_id.clone(),
        ts: c.timestamp_ms,
        prompt_length: a.int("prompt_length"),
        command_name: a.string("command_name"),
        command_source: a.string("command_source"),
    }))
}

fn permission_mode(a: &Attrs<'_>, c: &Common) -> Event {
    Event::PermissionModeChanged(Box::new(PermissionModeChange {
        session_id: c.session_id.clone(),
        from_mode: a.string("from_mode"),
        to_mode: a.string("to_mode"),
        trigger: a.string("trigger"),
        ts: c.timestamp_ms,
    }))
}

/// The session facts an OTLP event can supply.
///
/// Deliberately thin: the transcript lane owns cwd, git branch and the rest.
#[must_use]
pub fn session(c: &Common) -> Session {
    Session {
        session_id: c.session_id.clone().unwrap_or_default(),
        project_path: c.workspace_path.clone(),
        cc_version: c.app_version.clone(),
        first_seen: c.timestamp_ms,
        last_seen: c.timestamp_ms,
        ..Session::default()
    }
}

/// Drop the `claude_code.` qualifier if present.
fn strip_prefix(name: &str) -> String {
    name.strip_prefix("claude_code.")
        .unwrap_or(name)
        .to_string()
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

fn body_string(record: &LogRecord) -> Option<String> {
    use opentelemetry_proto::tonic::common::v1::any_value::Value as V;
    match record.body.as_ref()?.value.as_ref()? {
        V::StringValue(s) => Some(s.clone()),
        _ => None,
    }
}
