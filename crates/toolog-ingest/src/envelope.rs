//! The transcript record envelope.
//!
//! Every field is optional and unknown fields are ignored. The corpus spans 12
//! Claude Code versions and 21 record types, and a parser that insists on a
//! shape will meet one it does not know — at which point the right behaviour is
//! to skip the projection, never to fail the ingest ([ADR-0004]).
//!
//! [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md

use serde::Deserialize;
use serde_json::Value;

/// A transcript line, parsed as far as is safe to assume.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Envelope {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub uuid: Option<String>,
    pub parent_uuid: Option<String>,
    pub session_id: Option<String>,
    pub prompt_id: Option<String>,
    pub timestamp: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub version: Option<String>,
    pub is_sidechain: Option<bool>,
    pub entrypoint: Option<String>,
    pub slug: Option<String>,

    /// The permission mode in force — `default`, `auto`, `plan`, `dontAsk`.
    ///
    /// On a dedicated `permission-mode` record (which carries nothing but this,
    /// the type and the session id) and on `user` records at each prompt turn.
    /// **The OTLP lane does not carry this at all**, despite the plan assuming
    /// it did — see [`crate::projector`].
    pub permission_mode: Option<String>,

    /// Subagent *instance*. Present on every sidechain record and no
    /// main-thread record — the reliable discriminator.
    pub agent_id: Option<String>,
    /// Subagent *type* (`Explore`, …). Only on some records, so it is spread
    /// across an `agent_id` group during [`crate::projector`]'s finish pass.
    pub attribution_agent: Option<String>,

    /// On `agent-name` records: a *session* label like
    /// `host-password-reset-flow`. Unrelated to subagents.
    pub agent_name: Option<String>,
    /// On `relocated` records: where the session's working directory moved to.
    pub relocated_cwd: Option<String>,

    pub message: Option<Message>,
    /// Shape varies by tool: object, bare string, or array. All three occur.
    pub tool_use_result: Option<Value>,
}

/// The assistant/user message body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Message {
    pub role: Option<String>,
    /// A string for plain text, an array of blocks otherwise.
    pub content: Option<Value>,
}

/// A `tool_use` block from an assistant message.
#[derive(Debug, Clone)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
}

impl Envelope {
    /// Parse a transcript line, or `None` if it is not JSON at all.
    #[must_use]
    pub fn parse(line: &str) -> Option<Self> {
        serde_json::from_str(line).ok()
    }

    /// The record's `type`, or `""`.
    #[must_use]
    pub fn kind(&self) -> &str {
        self.kind.as_deref().unwrap_or_default()
    }

    /// Timestamp as milliseconds since the Unix epoch.
    pub fn timestamp_ms(&self) -> Option<i64> {
        self.timestamp
            .as_deref()?
            .parse::<jiff::Timestamp>()
            .ok()
            .map(jiff::Timestamp::as_millisecond)
    }

    /// Every `tool_use` block in this record.
    #[must_use]
    pub fn tool_uses(&self) -> Vec<ToolUse> {
        let Some(blocks) = self.content_blocks() else {
            return Vec::new();
        };
        blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .filter_map(|b| {
                Some(ToolUse {
                    id: b.get("id").and_then(Value::as_str)?.to_string(),
                    name: b.get("name").and_then(Value::as_str)?.to_string(),
                    input: b.get("input").cloned().unwrap_or(Value::Null),
                })
            })
            .collect()
    }

    /// The `tool_use_id` this record reports a result for, with the tool's own
    /// `is_error` flag when it set one.
    #[must_use]
    pub fn tool_result_ref(&self) -> Option<(String, Option<bool>)> {
        let blocks = self.content_blocks()?;
        blocks.iter().find_map(|b| {
            let id = b.get("tool_use_id").and_then(Value::as_str)?;
            Some((id.to_string(), b.get("is_error").and_then(Value::as_bool)))
        })
    }

    fn content_blocks(&self) -> Option<&Vec<Value>> {
        self.message.as_ref()?.content.as_ref()?.as_array()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_assistant_tool_use() {
        let line = r#"{"type":"assistant","uuid":"u1","parentUuid":"p1","sessionId":"s1",
            "timestamp":"2026-08-29T14:03:24.289Z","cwd":"/proj","gitBranch":"main",
            "version":"2.1.251","isSidechain":false,
            "message":{"role":"assistant","content":[
                {"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}}"#;
        let e = Envelope::parse(line).expect("parses");
        assert_eq!(e.kind(), "assistant");
        assert_eq!(e.session_id.as_deref(), Some("s1"));
        // 2026-08-29T14:03:24.289Z, computed independently of this parser.
        assert_eq!(e.timestamp_ms(), Some(1_788_012_204_289));

        let uses = e.tool_uses();
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].name, "Bash");
        assert_eq!(uses[0].input["command"], "ls");
    }

    #[test]
    fn parses_a_sidechain_record_with_agent_fields() {
        let line = r#"{"type":"assistant","sessionId":"s1","isSidechain":true,
            "agentId":"adad9906ec138f057","attributionAgent":"Explore",
            "slug":"plan-for-admin-users"}"#;
        let e = Envelope::parse(line).expect("parses");
        assert_eq!(e.agent_id.as_deref(), Some("adad9906ec138f057"));
        assert_eq!(e.attribution_agent.as_deref(), Some("Explore"));
        assert_eq!(e.is_sidechain, Some(true));
    }

    #[test]
    fn finds_a_tool_result_reference_and_its_error_flag() {
        let line = r#"{"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"toolu_9","is_error":true,"content":"boom"}]},
            "toolUseResult":"Error: boom"}"#;
        let e = Envelope::parse(line).expect("parses");
        assert_eq!(e.tool_result_ref(), Some(("toolu_9".into(), Some(true))));
    }

    /// The behaviour the whole module is built around.
    #[test]
    fn unknown_record_types_and_fields_parse_without_complaint() {
        let e = Envelope::parse(
            r#"{"type":"quantum-entangled-worktree","sessionId":"s1",
                "somethingFromTheFuture":{"deeply":["nested",1,true]}}"#,
        )
        .expect("unknown types still parse");
        assert_eq!(e.kind(), "quantum-entangled-worktree");
        assert_eq!(e.session_id.as_deref(), Some("s1"));
        assert!(e.tool_uses().is_empty());
    }

    #[test]
    fn non_json_returns_none_rather_than_panicking() {
        assert!(Envelope::parse("not json at all").is_none());
        assert!(Envelope::parse("").is_none());
    }

    #[test]
    fn a_string_message_content_is_not_mistaken_for_blocks() {
        let e = Envelope::parse(r#"{"type":"user","message":{"role":"user","content":"hello"}}"#)
            .expect("parses");
        assert!(e.tool_uses().is_empty());
        assert!(e.tool_result_ref().is_none());
    }
}
