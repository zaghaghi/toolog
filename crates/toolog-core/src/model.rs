//! Typed rows and the structs the lanes write through.
//!
//! The split between [`TranscriptFacts`] and [`OtelFacts`] is the type-level
//! expression of [ADR-0009]: each lane owns a disjoint set of columns, so
//! neither can clobber the other's regardless of arrival order.
//!
//! [ADR-0009]: ../../../docs/adr/0009-correlate-on-tool-use-id.md

use serde::{Deserialize, Serialize};

/// Which ingestion lane a record came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lane {
    /// `~/.claude/projects/**/*.jsonl` — the content of record.
    Transcript,
    /// Claude Code's OpenTelemetry export — decisions, durations, cost.
    Otlp,
}

impl Lane {
    /// The string stored in `raw_event.lane`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transcript => "transcript",
            Self::Otlp => "otlp",
        }
    }

    /// The `tool_call.provenance` bit this lane sets.
    #[must_use]
    pub fn provenance_bit(self) -> i64 {
        match self {
            Self::Transcript => provenance::TRANSCRIPT,
            Self::Otlp => provenance::OTLP,
        }
    }
}

/// Bits of `tool_call.provenance`.
///
/// A row carrying only [`TRANSCRIPT`](provenance::TRANSCRIPT) is a **gap in
/// collection**: the OTLP lane never saw it, so its decision, duration and cost
/// are missing. A row carrying only [`OTLP`](provenance::OTLP) is a call for
/// which no transcript body was written.
///
/// Neither bit means "rejected". Phase 4 measured a live denial and found it in
/// **both** lanes — see [`ToolCall::is_rejected`], which reads the decision.
pub mod provenance {
    /// Witnessed by the transcript lane.
    pub const TRANSCRIPT: i64 = 1;
    /// Witnessed by the OTLP lane.
    pub const OTLP: i64 = 2;
    /// Witnessed by both — the ordinary case for an accepted call.
    pub const BOTH: i64 = TRANSCRIPT | OTLP;
}

/// A record about to be written to the evidence store.
#[derive(Debug, Clone)]
pub struct NewRawEvent<'a> {
    pub lane: Lane,
    /// Transcript path, or an OTLP batch identifier.
    pub source_ref: &'a str,
    /// Byte offset within the transcript, for resume. `None` for OTLP.
    pub source_offset: Option<i64>,
    /// The record exactly as received. Never reformatted (ADR-0004).
    pub body: &'a str,
}

/// Outcome of an evidence-store insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawInsert {
    /// Stored, with its new `raw_event.id`.
    Inserted(i64),
    /// Already held — the content hash matched. Not an error: this is what
    /// makes a tailer's rescan-from-zero safe.
    Duplicate,
}

impl RawInsert {
    /// Whether this insert added a row.
    #[must_use]
    pub fn is_new(self) -> bool {
        matches!(self, Self::Inserted(_))
    }
}

/// A stored evidence record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub id: i64,
    pub lane: String,
    pub source_ref: String,
    pub source_offset: Option<i64>,
    pub content_sha256: String,
    pub ingested_at: i64,
    pub body: String,
}

/// Session facts, from transcript envelopes and OTLP attributes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Session {
    pub session_id: String,
    pub project_path: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub cc_version: Option<String>,
    pub entrypoint: Option<String>,
    /// A session label such as `host-password-reset-flow`, from `agent-name`
    /// records. **Not** subagent attribution — see [`ToolCall::agent_id`].
    pub agent_name: Option<String>,
    pub slug: Option<String>,
    pub first_seen: Option<i64>,
    pub last_seen: Option<i64>,
}

/// The columns the **transcript lane** owns.
///
/// OTEL truncates tool inputs at 512 characters, so it must never write these.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TranscriptFacts {
    pub session_id: Option<String>,
    pub prompt_id: Option<String>,
    pub message_uuid: Option<String>,
    pub parent_uuid: Option<String>,
    pub is_sidechain: Option<bool>,
    /// Subagent *instance*. Present on every sidechain record and no
    /// main-thread record, so it is the reliable discriminator.
    pub agent_id: Option<String>,
    /// Subagent *type* (`Explore`, `general-purpose`). Best-effort: it appears
    /// on only some records, and is backfilled per `agent_id`.
    pub agent_name: Option<String>,
    pub tool_name: Option<String>,
    pub tool_kind: Option<String>,
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
    pub called_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub input_json: Option<String>,
    pub input_summary: Option<String>,
    pub target_path: Option<String>,
    pub result_json: Option<String>,
    pub result_text: Option<String>,
    pub result_size: Option<i64>,
    pub success: Option<bool>,
    /// The permission mode in force when the call was made.
    ///
    /// Measured, not assumed: 987 OTLP records across 11 event types carried no
    /// `permission_mode` attribute and no `permission_mode_changed` event.
    /// Claude Code 2.1.260 puts the mode in the **transcript** — on a
    /// `permission-mode` record and on each `user` turn.
    pub permission_mode: Option<String>,
}

/// The columns the **OTLP lane** owns.
///
/// Transcripts record none of these — not the decision, not the duration, and
/// not the call at all when it was rejected.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OtelFacts {
    pub session_id: Option<String>,
    pub prompt_id: Option<String>,
    pub message_uuid: Option<String>,
    pub tool_name: Option<String>,
    pub tool_kind: Option<String>,
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
    pub called_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub error_type: Option<String>,
    pub decision: Option<String>,
    pub decision_source: Option<String>,
    pub success: Option<bool>,

    /// What the call was *asked* to do, from OTEL's `tool_input`.
    ///
    /// Written with **existing-wins** semantics, unlike every other field here:
    /// OTEL truncates values at 512 characters, so it must never displace a
    /// transcript's full copy. It fills in only where nothing else can: a call
    /// aborted before any transcript record was written, where this is the sole
    /// account of what was attempted.
    pub attempted_input_json: Option<String>,
    pub attempted_input_summary: Option<String>,
    pub attempted_target_path: Option<String>,
}

/// A fully assembled tool call.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct ToolCall {
    pub tool_use_id: String,
    pub session_id: Option<String>,
    pub prompt_id: Option<String>,
    pub message_uuid: Option<String>,
    pub parent_uuid: Option<String>,
    pub is_sidechain: Option<bool>,
    /// Subagent *instance*. Present on every sidechain record and no
    /// main-thread record, so it is the reliable discriminator.
    pub agent_id: Option<String>,
    /// Subagent *type* (`Explore`, `general-purpose`). Best-effort: it appears
    /// on only some records, and is backfilled per `agent_id`.
    pub agent_name: Option<String>,
    pub tool_name: Option<String>,
    pub tool_kind: Option<String>,
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
    pub called_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub input_json: Option<String>,
    pub input_summary: Option<String>,
    pub target_path: Option<String>,
    pub result_json: Option<String>,
    pub result_text: Option<String>,
    pub result_size: Option<i64>,
    pub success: Option<bool>,
    pub duration_ms: Option<i64>,
    pub error_type: Option<String>,
    pub decision: Option<String>,
    pub decision_source: Option<String>,
    pub permission_mode: Option<String>,
    pub provenance: i64,
}

impl ToolCall {
    /// Seen by the transcript lane.
    #[must_use]
    pub fn from_transcript(&self) -> bool {
        self.provenance & provenance::TRANSCRIPT != 0
    }

    /// Seen by the OTLP lane.
    #[must_use]
    pub fn from_otel(&self) -> bool {
        self.provenance & provenance::OTLP != 0
    }

    /// A call that was refused — by a permission rule or by a person.
    ///
    /// Read from `decision`, which only the OTLP lane supplies. Deliberately
    /// **not** inferred from provenance: a denial does leave a transcript
    /// record, it just says nothing about who denied it or why.
    #[must_use]
    pub fn is_rejected(&self) -> bool {
        self.decision.as_deref() == Some("reject")
    }
}

/// One file touched by an `Edit` or `Write`, from `structuredPatch`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct FileChange {
    pub tool_use_id: String,
    pub file_path: String,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub patch_json: Option<String>,
}

/// One API request. OTLP only — backfilled history has no cost data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiRequest {
    pub request_id: String,
    pub session_id: Option<String>,
    pub prompt_id: Option<String>,
    pub model: Option<String>,
    pub cost_usd_micros: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub duration_ms: Option<i64>,
    pub speed: Option<String>,
    pub effort: Option<String>,
    pub query_source: Option<String>,
    pub agent_name: Option<String>,
    pub ts: Option<i64>,
}

/// A prompt turn. Length and command name only — never the text (ADR-0008).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Prompt {
    pub prompt_id: String,
    pub session_id: Option<String>,
    pub ts: Option<i64>,
    pub prompt_length: Option<i64>,
    pub command_name: Option<String>,
    pub command_source: Option<String>,
}

/// A mid-session permission mode change. Feeds the Phase 6 risk rules.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionModeChange {
    pub session_id: Option<String>,
    pub from_mode: Option<String>,
    pub to_mode: Option<String>,
    pub trigger: Option<String>,
    pub ts: Option<i64>,
}

/// Filters for a timeline page. All fields are `AND`-ed; `None` means no
/// constraint.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct TimelineFilter {
    pub session_id: Option<String>,
    /// Only calls whose session the store never learned.
    ///
    /// A group the timeline can show but `session_id` cannot name, and without
    /// it "expand that group" would quietly widen to every row.
    pub session_unknown: Option<bool>,
    pub project_path: Option<String>,
    pub tool_name: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub success: Option<bool>,
    pub is_sidechain: Option<bool>,
    pub decision: Option<String>,
    pub decision_source: Option<String>,
    pub permission_mode: Option<String>,
    /// One subagent instance.
    pub agent_id: Option<String>,
    /// `true`: only main-thread calls. `false`: only subagent calls.
    ///
    /// Reads `agent_id`, which is present on every sidechain call and no
    /// main-thread one, rather than `is_sidechain`, which is null on any row
    /// the transcript lane has not witnessed.
    pub main_thread: Option<bool>,
    /// Free text, as typed. Sanitized into an FTS5 expression before use —
    /// `rm -rf` is a search term here, not an operator.
    pub query: Option<String>,
    /// Rows witnessed by exactly this set of lanes.
    ///
    /// Exact, not a mask. A mask can say "the OTLP lane saw this" but not "and
    /// the transcript lane did not", and it was a mask that let `--rejected`
    /// quietly mean "every call OTEL witnessed" for two phases.
    pub provenance: Option<i64>,
    /// Calls flagged by any live rule of this severity (task 12.7).
    ///
    /// The first two fields here that **cannot be compiled from the store
    /// alone**: a rule lives in a file, so the query layer is handed the rule
    /// set rather than looking it up. That also means a link carrying this is
    /// evaluated against the rules in force when it is *opened* — see
    /// [`crate::query::selection`].
    pub risk: Option<String>,
    /// Calls one named rule matched.
    pub rule_id: Option<String>,
    /// Free text over the local model's intent summaries (task 13.15).
    ///
    /// Like `risk` and `rule_id`, this cannot be compiled from the store alone:
    /// a verdict is keyed on which model and which prompt produced it, and the
    /// query layer is handed that pair rather than choosing one. **Not** the
    /// same index as `query`: that searches the command and its output, this
    /// searches what a model said the command was *for*, and "when did the
    /// agent start doing network things" is only answerable in the second.
    pub intent: Option<String>,
    /// Calls the local model scored, as a comparison: `>=4`, `<2`, `5`.
    ///
    /// A number on a scale rather than a word from a closed set, which is why
    /// it takes an operator where `risk` does not — and why it is deliberately
    /// a separate field from `risk`. An LLM score is not a rule severity, and a
    /// filter that could mix them would be the first step towards a view that
    /// does.
    pub llm_risk: Option<String>,
}

/// Paging for a timeline query.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Page {
    pub limit: u32,
    pub offset: u32,
}

impl Default for Page {
    fn default() -> Self {
        Self {
            limit: 100,
            offset: 0,
        }
    }
}

/// A search hit: the call, plus an FTS `snippet()` with matches marked.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct SearchHit {
    pub tool_call: ToolCall,
    pub snippet: String,
}

/// How completely the two lanes agree, per [ADR-0009].
///
/// This is what lets the tool state its own completeness rather than assume it.
/// Promoted to a full report by `toolog verify` in Phase 7.
///
/// [ADR-0009]: ../../../docs/adr/0009-correlate-on-tool-use-id.md
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Reconciliation {
    /// Seen by both lanes.
    pub both: i64,
    /// Transcript only: the OTLP lane missed it. A gap in collection.
    pub transcript_only: i64,
    /// OTLP only: no transcript body was ever written for this call.
    pub otel_only: i64,
    /// Calls the OTLP lane recorded a refusal for.
    ///
    /// Counted from `decision`, not from provenance. Phase 4 measured a real
    /// denial and found it in **both** lanes: the transcript keeps the
    /// `tool_use` block and a `tool_result` whose body is the refusal message.
    /// Only OTEL says *who* refused and *why*, as structured data — so a
    /// rejection is identified by this column, never by a missing transcript.
    pub rejected: i64,
}

impl Reconciliation {
    /// Total calls known from either lane.
    #[must_use]
    pub fn total(&self) -> i64 {
        self.both + self.transcript_only + self.otel_only
    }

    /// Share of transcript-witnessed calls the OTLP lane also saw.
    ///
    /// `None` when there is nothing to reconcile. The denominator is the calls
    /// the transcript witnessed, so an OTLP-only row neither helps nor hurts:
    /// it is a call with no transcript body, not a lane that fell behind.
    #[must_use]
    pub fn completeness(&self) -> Option<f64> {
        let witnessed = self.both + self.transcript_only;
        if witnessed == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        Some(self.both as f64 / witnessed as f64)
    }
}
