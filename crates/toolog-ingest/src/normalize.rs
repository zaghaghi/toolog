//! Per-tool normalizers: turning a tool's arguments and result into the columns
//! the timeline, filters and search actually use.
//!
//! Two rules govern everything here.
//!
//! **Unknown tools must degrade, never fail.** New tools ship constantly, and
//! the corpus already spans 12 Claude Code versions. Anything unrecognised
//! falls through to [`generic`] and still produces a usable row.
//!
//! **`toolUseResult` has three shapes and all of them occur**: an object
//! (2,171 times in the planning corpus), a bare string (99), and an array (62).
//! The bare string is always an error message; the array is an MCP content
//! block list.

use std::fmt::Write as _;

use serde_json::{Map, Value};
use toolog_core::model::FileChange;

/// Longest `input_summary` kept. Enough for a real shell command; short enough
/// that the timeline stays scannable.
const SUMMARY_LIMIT: usize = 500;

/// Base64 payloads above this are elided from the *projection*. The evidence in
/// `raw_event` keeps them.
const INLINE_BINARY_LIMIT: usize = 1024;

/// What a tool call was asked to do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputFacts {
    /// The one-line display string: the command, the path, the URL.
    pub summary: Option<String>,
    /// A file path or URL, for path filtering.
    pub target: Option<String>,
}

/// What a tool call produced.
#[derive(Debug, Clone, Default)]
pub struct ResultFacts {
    /// Plain text for full-text search, with binary elided.
    pub text: String,
    pub success: Option<bool>,
    pub error_type: Option<String>,
}

/// How a tool is provided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Builtin,
    Mcp,
    Agent,
    Skill,
}

impl ToolKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Mcp => "mcp",
            Self::Agent => "agent",
            Self::Skill => "skill",
        }
    }
}

/// Classify a tool name, splitting `mcp__server__tool` into its parts.
#[must_use]
pub fn classify(name: &str) -> (ToolKind, Option<String>, Option<String>) {
    if let Some(rest) = name.strip_prefix("mcp__") {
        let (server, tool) = rest.split_once("__").map_or((rest, ""), |(s, t)| (s, t));
        return (
            ToolKind::Mcp,
            Some(server.to_string()),
            (!tool.is_empty()).then(|| tool.to_string()),
        );
    }
    let kind = match name {
        "Agent" | "Task" => ToolKind::Agent,
        "Skill" => ToolKind::Skill,
        _ => ToolKind::Builtin,
    };
    (kind, None, None)
}

/// Summarize a tool's arguments for display and filtering.
///
/// Dispatch is by tool name, with [`generic`] as the fallback that makes an
/// unrecognised tool degrade instead of erroring.
pub fn input(tool: &str, input: &Value) -> InputFacts {
    let s = |key: &str| input.get(key).and_then(Value::as_str);

    match tool {
        "Bash" => InputFacts {
            summary: s("command").map(truncate),
            target: None,
        },
        "Read" | "Write" | "NotebookEdit" => InputFacts {
            summary: s("file_path").or_else(|| s("notebook_path")).map(truncate),
            target: s("file_path")
                .or_else(|| s("notebook_path"))
                .map(str::to_string),
        },
        "Edit" => InputFacts {
            summary: s("file_path").map(truncate),
            target: s("file_path").map(str::to_string),
        },
        "WebFetch" => InputFacts {
            summary: s("url").map(truncate),
            target: s("url").map(str::to_string),
        },
        "WebSearch" => InputFacts {
            summary: s("query").map(truncate),
            target: None,
        },
        "Glob" | "Grep" => InputFacts {
            summary: s("pattern").map(truncate),
            target: s("path").map(str::to_string),
        },
        // The agent's own description reads far better in a timeline than its
        // multi-paragraph prompt.
        "Agent" | "Task" => InputFacts {
            summary: s("description").or_else(|| s("prompt")).map(truncate),
            target: s("subagent_type").map(str::to_string),
        },
        "Skill" => InputFacts {
            summary: s("skill").or_else(|| s("command")).map(truncate),
            target: s("skill").map(str::to_string),
        },
        _ => generic(input),
    }
}

/// The fallback for tools this build has never heard of.
///
/// Tries the keys tools conventionally use, then falls back to compact JSON, so
/// a brand-new tool still produces a readable row.
pub fn generic(input: &Value) -> InputFacts {
    const LIKELY: &[&str] = &[
        "command",
        "file_path",
        "path",
        "url",
        "query",
        "pattern",
        "description",
        "prompt",
    ];

    for key in LIKELY {
        if let Some(v) = input.get(*key).and_then(Value::as_str) {
            let target = matches!(*key, "file_path" | "path" | "url").then(|| v.to_string());
            return InputFacts {
                summary: Some(truncate(v)),
                target,
            };
        }
    }

    match input {
        Value::Null => InputFacts::default(),
        Value::String(s) => InputFacts {
            summary: Some(truncate(s)),
            target: None,
        },
        other => InputFacts {
            summary: Some(truncate(&other.to_string())),
            target: None,
        },
    }
}

/// Extract searchable text, success and error kind from a `toolUseResult`.
///
/// `is_error` comes from the `tool_result` block when the tool set it, and is
/// trusted over anything inferred from the payload.
#[must_use]
pub fn result(value: &Value, is_error: Option<bool>) -> ResultFacts {
    let mut facts = match value {
        // A bare string is always an error. Every one of the 99 in the planning
        // corpus began "Error: ".
        Value::String(s) => ResultFacts {
            text: s.clone(),
            success: Some(false),
            error_type: Some(error_type(s)),
        },
        Value::Array(items) => ResultFacts {
            text: content_blocks_text(items),
            success: None,
            error_type: None,
        },
        Value::Object(map) => object_result(map),
        _ => ResultFacts::default(),
    };

    if let Some(true) = is_error {
        facts.success = Some(false);
        if facts.error_type.is_none() {
            facts.error_type = Some(error_type(&facts.text));
        }
    } else if is_error == Some(false) && facts.success.is_none() {
        facts.success = Some(true);
    }
    facts
}

fn object_result(map: &Map<String, Value>) -> ResultFacts {
    let mut text = String::new();
    for key in [
        "stdout", "stderr", "content", "text", "result", "codeText", "query",
    ] {
        if let Some(s) = map.get(key).and_then(Value::as_str)
            && !s.is_empty()
        {
            text.push_str(s);
            text.push('\n');
        }
    }
    // Read results nest the body one level down.
    if let Some(s) = map
        .get("file")
        .and_then(|f| f.get("content"))
        .and_then(Value::as_str)
    {
        text.push_str(s);
    }
    // MCP-style content arrays can appear inside an object too.
    if let Some(items) = map.get("content").and_then(Value::as_array) {
        text.push_str(&content_blocks_text(items));
    }

    let interrupted = map
        .get("interrupted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let success = if interrupted { Some(false) } else { Some(true) };
    let error_type = interrupted.then(|| "Interrupted".to_string());

    ResultFacts {
        text,
        success,
        error_type,
    }
}

/// Join the text of MCP content blocks, naming binary rather than inlining it.
fn content_blocks_text(items: &[Value]) -> String {
    let mut out = String::new();
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = item.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                    out.push('\n');
                }
            }
            Some("image") => {
                let media = item
                    .pointer("/source/media_type")
                    .and_then(Value::as_str)
                    .unwrap_or("image");
                let bytes = item
                    .pointer("/source/data")
                    .and_then(Value::as_str)
                    .map_or(0, str::len);
                let _ = writeln!(out, "[{media}, {bytes} bytes]");
            }
            _ => {
                if let Some(t) = item.as_str() {
                    out.push_str(t);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// Categorize an error message, e.g. `Error: ENOENT ...` -> `ENOENT`.
fn error_type(text: &str) -> String {
    let rest = text.strip_prefix("Error: ").unwrap_or(text);
    let first = rest.lines().next().unwrap_or(rest).trim();
    first
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(64)
        .collect()
}

/// Copy a result with large base64 payloads replaced by a description.
///
/// The evidence in `raw_event` keeps the original ([ADR-0004]); this keeps a
/// single screenshot from dominating the projection. In the planning corpus the
/// largest stored result was a 610 KiB base64 JPEG.
///
/// [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md
pub fn elide_binary(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(elide_binary).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                let elided = match v {
                    Value::String(s) if k == "data" && s.len() > INLINE_BINARY_LIMIT => {
                        Value::String(format!("[elided: {} bytes of base64]", s.len()))
                    }
                    other => elide_binary(other),
                };
                out.insert(k.clone(), elided);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Files touched by an `Edit` or `Write`, from `structuredPatch`.
///
/// Hunk lines are prefixed `+`, `-` or a space, so added and removed counts come
/// straight off the prefixes. A `Write` that creates a file has an empty patch
/// but still produced a change, and is reported with zero counts.
pub fn file_changes(tool_use_id: &str, result: &Value) -> Vec<FileChange> {
    let Some(map) = result.as_object() else {
        return Vec::new();
    };
    let Some(target) = map.get("filePath").and_then(Value::as_str) else {
        return Vec::new();
    };

    let mut added = 0i64;
    let mut removed = 0i64;
    let patch = map.get("structuredPatch").and_then(Value::as_array);

    for hunk in patch.into_iter().flatten() {
        for line in hunk
            .get("lines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match line.as_str().and_then(|s| s.chars().next()) {
                Some('+') => added += 1,
                Some('-') => removed += 1,
                _ => {}
            }
        }
    }

    vec![FileChange {
        tool_use_id: tool_use_id.to_string(),
        file_path: target.to_string(),
        lines_added: added,
        lines_removed: removed,
        patch_json: patch.map(|p| Value::Array(p.clone()).to_string()),
    }]
}

fn truncate(s: &str) -> String {
    let cleaned = s.trim();
    if cleaned.chars().count() <= SUMMARY_LIMIT {
        return cleaned.to_string();
    }
    let mut out: String = cleaned.chars().take(SUMMARY_LIMIT).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_builtin_mcp_agent_and_skill() {
        assert_eq!(classify("Bash").0, ToolKind::Builtin);
        assert_eq!(classify("Agent").0, ToolKind::Agent);
        assert_eq!(classify("Skill").0, ToolKind::Skill);

        let (kind, server, tool) = classify("mcp__Claude_Preview__preview_eval");
        assert_eq!(kind, ToolKind::Mcp);
        assert_eq!(server.as_deref(), Some("Claude_Preview"));
        assert_eq!(tool.as_deref(), Some("preview_eval"));
    }

    #[test]
    fn a_malformed_mcp_name_is_still_recognised_as_mcp() {
        let (kind, server, tool) = classify("mcp__lonely");
        assert_eq!(kind, ToolKind::Mcp);
        assert_eq!(server.as_deref(), Some("lonely"));
        assert_eq!(tool, None);
    }

    #[test]
    fn known_tools_summarize_to_their_most_useful_field() {
        assert_eq!(
            input("Bash", &json!({"command": "ls -la", "description": "list"}))
                .summary
                .as_deref(),
            Some("ls -la")
        );
        let read = input("Read", &json!({"file_path": "/a/b.rs"}));
        assert_eq!(read.summary.as_deref(), Some("/a/b.rs"));
        assert_eq!(read.target.as_deref(), Some("/a/b.rs"));

        // An agent's description reads better in a timeline than its prompt.
        let agent = input(
            "Agent",
            &json!({"description": "Map auth", "subagent_type": "Explore", "prompt": "long..."}),
        );
        assert_eq!(agent.summary.as_deref(), Some("Map auth"));
        assert_eq!(agent.target.as_deref(), Some("Explore"));
    }

    #[test]
    fn an_unknown_tool_still_produces_a_row() {
        let known_key = input("SomethingNew", &json!({"file_path": "/x.rs", "depth": 3}));
        assert_eq!(known_key.target.as_deref(), Some("/x.rs"));

        let alien = input("SomethingNew", &json!({"entanglement": 7}));
        assert!(alien.summary.expect("summary").contains("entanglement"));

        assert_eq!(input("Whatever", &Value::Null), InputFacts::default());
    }

    #[test]
    fn summaries_are_truncated_but_never_split_a_character() {
        let long = "é".repeat(SUMMARY_LIMIT + 50);
        let out = input("Bash", &json!({ "command": long }))
            .summary
            .expect("summary");
        assert_eq!(
            out.chars().count(),
            SUMMARY_LIMIT + 1,
            "limit plus the ellipsis"
        );
        assert!(out.ends_with('…'));
    }

    /// A bare-string result is always an error — every one in the planning
    /// corpus began "Error: ".
    #[test]
    fn a_string_result_is_an_error() {
        let facts = result(&json!("Error: ENOENT no such file"), None);
        assert_eq!(facts.success, Some(false));
        assert_eq!(facts.error_type.as_deref(), Some("ENOENT no such"));
        assert!(facts.text.contains("ENOENT"));
    }

    #[test]
    fn an_object_result_reads_stdout_and_respects_interruption() {
        let ok = result(
            &json!({"stdout": "done", "stderr": "", "interrupted": false}),
            None,
        );
        assert_eq!(ok.success, Some(true));
        assert_eq!(ok.text.trim(), "done");

        let stopped = result(&json!({"stdout": "", "interrupted": true}), None);
        assert_eq!(stopped.success, Some(false));
        assert_eq!(stopped.error_type.as_deref(), Some("Interrupted"));
    }

    #[test]
    fn a_read_result_finds_the_nested_file_content() {
        let facts = result(
            &json!({"type": "text", "file": {"content": "fn main() {}"}}),
            None,
        );
        assert!(facts.text.contains("fn main()"));
    }

    #[test]
    fn an_array_result_joins_text_and_names_images() {
        let facts = result(
            &json!([
                {"type": "text", "text": "Captured."},
                {"type": "image", "source": {"media_type": "image/png", "data": "AAAA"}}
            ]),
            None,
        );
        assert!(facts.text.contains("Captured."));
        assert!(
            facts.text.contains("image/png"),
            "the image is named, not inlined"
        );
        assert!(!facts.text.contains("AAAA"));
    }

    #[test]
    fn the_tools_own_error_flag_wins_over_inference() {
        let facts = result(
            &json!({"stdout": "looks fine", "interrupted": false}),
            Some(true),
        );
        assert_eq!(facts.success, Some(false));
        assert!(facts.error_type.is_some());
    }

    #[test]
    fn large_base64_is_elided_and_small_values_are_left_alone() {
        let big = "A".repeat(INLINE_BINARY_LIMIT + 1);
        let elided = elide_binary(&json!({"source": {"data": big, "media_type": "image/png"}}));
        let data = elided
            .pointer("/source/data")
            .and_then(Value::as_str)
            .expect("data");
        assert!(data.starts_with("[elided:"));
        assert_eq!(
            elided.pointer("/source/media_type").and_then(Value::as_str),
            Some("image/png")
        );

        let small = json!({"data": "short"});
        assert_eq!(elide_binary(&small), small, "small values are untouched");
    }

    #[test]
    fn structured_patches_become_line_counts() {
        let changes = file_changes(
            "toolu_1",
            &json!({
                "filePath": "/a/b.rs",
                "structuredPatch": [{
                    "oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 3,
                    "lines": ["-old line", "+new one", "+new two", " context"]
                }]
            }),
        );
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].lines_added, 2);
        assert_eq!(changes[0].lines_removed, 1);
        assert!(changes[0].patch_json.is_some());
    }

    #[test]
    fn a_created_file_has_an_empty_patch_but_is_still_a_change() {
        let changes = file_changes(
            "toolu_1",
            &json!({"filePath": "/new.md", "structuredPatch": []}),
        );
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].lines_added, 0);

        // No path means no file change at all.
        assert!(file_changes("toolu_1", &json!({"stdout": "x"})).is_empty());
        assert!(file_changes("toolu_1", &json!("Error: nope")).is_empty());
    }
}
