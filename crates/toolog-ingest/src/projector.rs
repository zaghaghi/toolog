//! Turning transcript evidence into projection rows.
//!
//! Implements [`toolog_core::project::Projector`], so the same code serves both
//! backfill and the live tail, and re-projection from `raw_event` gets the
//! identical result.
//!
//! # Subagent attribution
//!
//! The plan assumed `agent-name` records identified subagents. Profiling the
//! real corpus showed otherwise, and three separate things had been conflated:
//!
//! - **`agentId`** — a subagent *instance*. On 100% of sidechain records
//!   (658/658) and 0% of main-thread ones (0/5944). The reliable discriminator.
//! - **`attributionAgent`** — the subagent *type* (`Explore`). On only ~57% of
//!   sidechain records, so it is spread across an `agentId` group in [`finish`].
//! - **`agent-name`** — a *session* label like `host-password-reset-flow`. A
//!   session carrying one may have no sidechain records at all, which is how
//!   the conflation was caught.
//!
//! The authoritative link is the spawning `Agent` call: its result carries the
//! same `agentId` its sidechain records do, and its input carries
//! `subagent_type`.
//!
//! [`finish`]: TranscriptProjector::finish

use std::collections::HashMap;

use serde_json::Value;
use toolog_core::model::{RawEvent, Session, TranscriptFacts};
use toolog_core::project::{self, Projector};
use toolog_core::{Connection, Result, raw};

use crate::envelope::Envelope;
use toolog_core::normalize;

/// Counts of what a projection run saw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectStats {
    pub records: usize,
    pub tool_uses: usize,
    pub tool_results: usize,
    pub sessions: usize,
    /// Records whose `type` this build does not project. Counted and reported,
    /// never fatal.
    pub unknown_records: usize,
    /// Lines that were not JSON at all.
    pub unparsable: usize,
}

/// Projects transcript records into `toolog-core` tables.
#[derive(Debug, Default)]
pub struct TranscriptProjector {
    stats: ProjectStats,
    /// `agentId` -> subagent type, learned from `Agent` results and from
    /// `attributionAgent`, then spread across each group in [`Self::finish`].
    agent_types: HashMap<String, String>,
    /// `tool_use_id` of an `Agent` call -> its `subagent_type`, held until the
    /// matching result reveals the `agentId`.
    pending_agent_types: HashMap<String, String>,
    seen_sessions: std::collections::HashSet<String>,
    /// `session_id` -> relocated cwd, applied in [`Self::finish`].
    relocations: HashMap<String, String>,
}

impl TranscriptProjector {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn stats(&self) -> &ProjectStats {
        &self.stats
    }

    fn project_envelope(
        &mut self,
        conn: &Connection,
        e: &Envelope,
        source_ref: &str,
    ) -> Result<()> {
        self.stats.records += 1;
        let ts = e.timestamp_ms();

        if let Some(sid) = e.session_id.clone() {
            if self.seen_sessions.insert(sid.clone()) {
                self.stats.sessions += 1;
            }
            if let Some(moved) = &e.relocated_cwd {
                self.relocations.insert(sid.clone(), moved.clone());
            }
            let cwd = e.relocated_cwd.clone().or_else(|| e.cwd.clone());
            project::upsert_session(
                conn,
                &Session {
                    session_id: sid,
                    project_path: cwd.clone(),
                    transcript_path: Some(source_ref.to_string()),
                    cwd,
                    git_branch: e.git_branch.clone(),
                    cc_version: e.version.clone(),
                    entrypoint: e.entrypoint.clone(),
                    // Only ever from an `agent-name` record — a session label.
                    agent_name: e.agent_name.clone(),
                    slug: e.slug.clone(),
                    first_seen: ts,
                    last_seen: ts,
                },
            )?;
        }

        if let Some(id) = &e.agent_id
            && let Some(kind) = &e.attribution_agent
        {
            self.agent_types.insert(id.clone(), kind.clone());
        }

        match e.kind() {
            "assistant" => self.project_tool_uses(conn, e, ts)?,
            "user" => self.project_tool_result(conn, e, ts)?,
            "agent-name" | "relocated" => {} // handled by the session upsert above
            _ => self.stats.unknown_records += 1,
        }
        Ok(())
    }

    fn project_tool_uses(
        &mut self,
        conn: &Connection,
        e: &Envelope,
        ts: Option<i64>,
    ) -> Result<()> {
        for use_ in e.tool_uses() {
            self.stats.tool_uses += 1;
            let (kind, server, tool) = normalize::classify(&use_.name);
            let facts = normalize::input(&use_.name, &use_.input);

            // Remember the requested subagent type; the result carries the
            // agentId that ties it to the sidechain calls.
            if kind == normalize::ToolKind::Agent
                && let Some(t) = use_.input.get("subagent_type").and_then(Value::as_str)
            {
                self.pending_agent_types
                    .insert(use_.id.clone(), t.to_string());
            }

            project::upsert_transcript(
                conn,
                &use_.id,
                &TranscriptFacts {
                    session_id: e.session_id.clone(),
                    prompt_id: e.prompt_id.clone(),
                    message_uuid: e.uuid.clone(),
                    parent_uuid: e.parent_uuid.clone(),
                    is_sidechain: e.is_sidechain,
                    agent_id: e.agent_id.clone(),
                    agent_name: e.attribution_agent.clone(),
                    tool_name: Some(use_.name.clone()),
                    tool_kind: Some(kind.as_str().to_string()),
                    mcp_server: server,
                    mcp_tool: tool,
                    called_at: ts,
                    input_json: Some(use_.input.to_string()),
                    input_summary: facts.summary,
                    target_path: facts.target,
                    ..TranscriptFacts::default()
                },
            )?;
        }
        Ok(())
    }

    fn project_tool_result(
        &mut self,
        conn: &Connection,
        e: &Envelope,
        ts: Option<i64>,
    ) -> Result<()> {
        let Some(result) = &e.tool_use_result else {
            self.stats.unknown_records += 1;
            return Ok(());
        };
        let Some((tool_use_id, is_error)) = e.tool_result_ref() else {
            return Ok(());
        };
        self.stats.tool_results += 1;

        // An `Agent` result names the agentId its sidechain calls will carry.
        if let Some(agent_id) = result.get("agentId").and_then(Value::as_str)
            && let Some(kind) = self.pending_agent_types.remove(&tool_use_id)
        {
            self.agent_types.insert(agent_id.to_string(), kind);
        }

        let facts = normalize::result(result, is_error);
        let stored = normalize::elide_binary(result).to_string();

        project::upsert_transcript(
            conn,
            &tool_use_id,
            &TranscriptFacts {
                session_id: e.session_id.clone(),
                agent_id: e.agent_id.clone(),
                completed_at: ts,
                result_size: Some(i64::try_from(stored.len()).unwrap_or(i64::MAX)),
                result_json: Some(stored),
                result_text: Some(facts.text),
                success: facts.success,
                ..TranscriptFacts::default()
            },
        )?;

        for change in normalize::file_changes(&tool_use_id, result) {
            project::insert_file_change(conn, &change)?;
        }
        Ok(())
    }
}

impl Projector for TranscriptProjector {
    fn project(&mut self, conn: &Connection, event: &RawEvent) -> Result<()> {
        // Only this lane's records. A re-projection replays the whole evidence
        // store through both projectors, so without this an OTLP record would
        // be counted as an unparsable transcript line.
        if event.lane != toolog_core::model::Lane::Transcript.as_str() {
            return Ok(());
        }
        if let Some(e) = Envelope::parse(&event.body) {
            self.project_envelope(conn, &e, &event.source_ref)
        } else {
            self.stats.unparsable += 1;
            Ok(())
        }
    }

    /// Settle the facts that need the whole stream: relocations and subagent
    /// types.
    ///
    /// `attributionAgent` is on only some records, so a call can know its
    /// `agent_id` while the type was learned from a sibling — or from the
    /// spawning `Agent` call, which may sit in a different record entirely.
    fn finish(&mut self, conn: &Connection) -> Result<()> {
        for (session_id, cwd) in &self.relocations {
            project::relocate_session(conn, session_id, cwd)?;
        }
        for (agent_id, kind) in &self.agent_types {
            project::set_agent_type(conn, agent_id, kind)?;
        }
        // Any instance whose type was never named still gets a consistent label
        // from whichever of its own records happened to carry one.
        project::spread_agent_names(conn)
    }
}

/// Store one transcript line as evidence, returning whether it was new.
pub fn store_line(conn: &Connection, source_ref: &str, offset: i64, line: &str) -> Result<bool> {
    let event = toolog_core::model::NewRawEvent {
        lane: toolog_core::model::Lane::Transcript,
        source_ref,
        source_offset: Some(offset),
        body: line,
    };
    Ok(raw::insert(conn, &event)?.is_new())
}
