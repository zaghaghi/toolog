//! Projection: turning stored evidence into queryable rows.
//!
//! Two ideas live here.
//!
//! **Order-independent upserts** ([ADR-0009]). Each lane writes only the columns
//! it owns, and merges rather than replaces. A call may be created by either
//! lane and completed by the other; the final row is the same either way. The
//! type system carries most of this — [`TranscriptFacts`] has no `duration_ms`
//! field and [`OtelFacts`] has no `input_json` field, so neither lane can
//! overwrite the other's evidence even by mistake.
//!
//! **Re-projection** ([ADR-0004]). Every projection table can be rebuilt from
//! `raw_event`. Parsing lives in the lane crates, so this module owns the
//! mechanism and takes a [`Projector`] for the meaning.
//!
//! [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md
//! [ADR-0009]: ../../../docs/adr/0009-correlate-on-tool-use-id.md

use rusqlite::{Connection, named_params, params};

use crate::error::Result;
use crate::model::{
    ApiRequest, FileChange, Lane, OtelFacts, PermissionModeChange, Prompt, RawEvent, Session,
    TranscriptFacts, provenance,
};
use crate::raw;

/// Projection tables, in an order safe to delete given the foreign keys.
const PROJECTION_TABLES: &[&str] = &[
    "file_change",
    "tool_call",
    "session",
    "api_request",
    "prompt",
    "permission_mode_change",
];

/// Merge session facts. Timestamps widen to cover everything seen.
pub fn upsert_session(conn: &Connection, s: &Session) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO session (session_id, project_path, transcript_path, cwd, git_branch,
                              cc_version, entrypoint, agent_name, first_seen, last_seen)
         VALUES (:session_id, :project_path, :transcript_path, :cwd, :git_branch,
                 :cc_version, :entrypoint, :agent_name, :first_seen, :last_seen)
         ON CONFLICT (session_id) DO UPDATE SET
             project_path    = COALESCE(excluded.project_path,    session.project_path),
             transcript_path = COALESCE(excluded.transcript_path, session.transcript_path),
             cwd             = COALESCE(excluded.cwd,             session.cwd),
             git_branch      = COALESCE(excluded.git_branch,      session.git_branch),
             cc_version      = COALESCE(excluded.cc_version,      session.cc_version),
             entrypoint      = COALESCE(excluded.entrypoint,      session.entrypoint),
             agent_name      = COALESCE(excluded.agent_name,      session.agent_name),
             first_seen      = min(COALESCE(excluded.first_seen, session.first_seen),
                                   COALESCE(session.first_seen, excluded.first_seen)),
             last_seen       = max(COALESCE(excluded.last_seen, session.last_seen),
                                   COALESCE(session.last_seen, excluded.last_seen))",
    )?
    .execute(named_params! {
        ":session_id": s.session_id,
        ":project_path": s.project_path,
        ":transcript_path": s.transcript_path,
        ":cwd": s.cwd,
        ":git_branch": s.git_branch,
        ":cc_version": s.cc_version,
        ":entrypoint": s.entrypoint,
        ":agent_name": s.agent_name,
        ":first_seen": s.first_seen,
        ":last_seen": s.last_seen,
    })?;
    Ok(())
}

/// Write the transcript lane's contribution to a tool call.
///
/// Sets [`provenance::TRANSCRIPT`]. Never touches `duration_ms`, `decision`,
/// `decision_source` or `error_type` — those belong to the OTLP lane.
pub fn upsert_transcript(conn: &Connection, tool_use_id: &str, f: &TranscriptFacts) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO tool_call (
             tool_use_id, session_id, prompt_id, message_uuid, parent_uuid, is_sidechain,
             agent_name, tool_name, tool_kind, mcp_server, mcp_tool, called_at, completed_at,
             input_json, input_summary, target_path, result_json, result_text, result_size,
             success, provenance)
         VALUES (
             :tool_use_id, :session_id, :prompt_id, :message_uuid, :parent_uuid, :is_sidechain,
             :agent_name, :tool_name, :tool_kind, :mcp_server, :mcp_tool, :called_at, :completed_at,
             :input_json, :input_summary, :target_path, :result_json, :result_text, :result_size,
             :success, :bit)
         ON CONFLICT (tool_use_id) DO UPDATE SET
             session_id    = COALESCE(excluded.session_id,    tool_call.session_id),
             prompt_id     = COALESCE(excluded.prompt_id,     tool_call.prompt_id),
             message_uuid  = COALESCE(excluded.message_uuid,  tool_call.message_uuid),
             parent_uuid   = COALESCE(excluded.parent_uuid,   tool_call.parent_uuid),
             is_sidechain  = COALESCE(excluded.is_sidechain,  tool_call.is_sidechain),
             agent_name    = COALESCE(excluded.agent_name,    tool_call.agent_name),
             tool_name     = COALESCE(excluded.tool_name,     tool_call.tool_name),
             tool_kind     = COALESCE(excluded.tool_kind,     tool_call.tool_kind),
             mcp_server    = COALESCE(excluded.mcp_server,    tool_call.mcp_server),
             mcp_tool      = COALESCE(excluded.mcp_tool,      tool_call.mcp_tool),
             called_at     = COALESCE(excluded.called_at,     tool_call.called_at),
             completed_at  = COALESCE(excluded.completed_at,  tool_call.completed_at),
             input_json    = COALESCE(excluded.input_json,    tool_call.input_json),
             input_summary = COALESCE(excluded.input_summary, tool_call.input_summary),
             target_path   = COALESCE(excluded.target_path,   tool_call.target_path),
             result_json   = COALESCE(excluded.result_json,   tool_call.result_json),
             result_text   = COALESCE(excluded.result_text,   tool_call.result_text),
             result_size   = COALESCE(excluded.result_size,   tool_call.result_size),
             success       = COALESCE(excluded.success,       tool_call.success),
             provenance    = tool_call.provenance | :bit",
    )?
    .execute(named_params! {
        ":tool_use_id": tool_use_id,
        ":session_id": f.session_id,
        ":prompt_id": f.prompt_id,
        ":message_uuid": f.message_uuid,
        ":parent_uuid": f.parent_uuid,
        ":is_sidechain": f.is_sidechain,
        ":agent_name": f.agent_name,
        ":tool_name": f.tool_name,
        ":tool_kind": f.tool_kind,
        ":mcp_server": f.mcp_server,
        ":mcp_tool": f.mcp_tool,
        ":called_at": f.called_at,
        ":completed_at": f.completed_at,
        ":input_json": f.input_json,
        ":input_summary": f.input_summary,
        ":target_path": f.target_path,
        ":result_json": f.result_json,
        ":result_text": f.result_text,
        ":result_size": f.result_size,
        ":success": f.success,
        ":bit": provenance::TRANSCRIPT,
    })?;
    Ok(())
}

/// Write the OTLP lane's contribution to a tool call.
///
/// Sets [`provenance::OTLP`]. Never touches `input_json`, `result_json` or the
/// summaries — OTEL truncates tool inputs at 512 characters, and a truncated
/// command must never overwrite the real one.
///
/// When no transcript row exists this creates one. That is not a defect: it is
/// how a **rejected** call enters the database, since a denied call leaves no
/// transcript trace at all.
pub fn upsert_otel(conn: &Connection, tool_use_id: &str, f: &OtelFacts) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO tool_call (
             tool_use_id, session_id, prompt_id, message_uuid, tool_name, tool_kind,
             mcp_server, mcp_tool, called_at, duration_ms, error_type, decision,
             decision_source, permission_mode, success, provenance)
         VALUES (
             :tool_use_id, :session_id, :prompt_id, :message_uuid, :tool_name, :tool_kind,
             :mcp_server, :mcp_tool, :called_at, :duration_ms, :error_type, :decision,
             :decision_source, :permission_mode, :success, :bit)
         ON CONFLICT (tool_use_id) DO UPDATE SET
             session_id      = COALESCE(excluded.session_id,      tool_call.session_id),
             prompt_id       = COALESCE(excluded.prompt_id,       tool_call.prompt_id),
             message_uuid    = COALESCE(excluded.message_uuid,    tool_call.message_uuid),
             tool_name       = COALESCE(excluded.tool_name,       tool_call.tool_name),
             tool_kind       = COALESCE(excluded.tool_kind,       tool_call.tool_kind),
             mcp_server      = COALESCE(excluded.mcp_server,      tool_call.mcp_server),
             mcp_tool        = COALESCE(excluded.mcp_tool,        tool_call.mcp_tool),
             called_at       = COALESCE(excluded.called_at,       tool_call.called_at),
             duration_ms     = COALESCE(excluded.duration_ms,     tool_call.duration_ms),
             error_type      = COALESCE(excluded.error_type,      tool_call.error_type),
             decision        = COALESCE(excluded.decision,        tool_call.decision),
             decision_source = COALESCE(excluded.decision_source, tool_call.decision_source),
             permission_mode = COALESCE(excluded.permission_mode, tool_call.permission_mode),
             success         = COALESCE(excluded.success,         tool_call.success),
             provenance      = tool_call.provenance | :bit",
    )?
    .execute(named_params! {
        ":tool_use_id": tool_use_id,
        ":session_id": f.session_id,
        ":prompt_id": f.prompt_id,
        ":message_uuid": f.message_uuid,
        ":tool_name": f.tool_name,
        ":tool_kind": f.tool_kind,
        ":mcp_server": f.mcp_server,
        ":mcp_tool": f.mcp_tool,
        ":called_at": f.called_at,
        ":duration_ms": f.duration_ms,
        ":error_type": f.error_type,
        ":decision": f.decision,
        ":decision_source": f.decision_source,
        ":permission_mode": f.permission_mode,
        ":success": f.success,
        ":bit": provenance::OTLP,
    })?;
    Ok(())
}

/// Record one file touched by an `Edit` or `Write`.
pub fn insert_file_change(conn: &Connection, c: &FileChange) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO file_change (tool_use_id, file_path, lines_added, lines_removed, patch_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![
        c.tool_use_id,
        c.file_path,
        c.lines_added,
        c.lines_removed,
        c.patch_json
    ])?;
    Ok(())
}

/// Merge one API request. OTLP only.
pub fn upsert_api_request(conn: &Connection, r: &ApiRequest) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO api_request (
             request_id, session_id, prompt_id, model, cost_usd_micros, input_tokens,
             output_tokens, cache_read_tokens, cache_creation_tokens, duration_ms,
             speed, effort, query_source, agent_name, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT (request_id) DO UPDATE SET
             cost_usd_micros = COALESCE(excluded.cost_usd_micros, api_request.cost_usd_micros),
             duration_ms     = COALESCE(excluded.duration_ms,     api_request.duration_ms)",
    )?
    .execute(params![
        r.request_id,
        r.session_id,
        r.prompt_id,
        r.model,
        r.cost_usd_micros,
        r.input_tokens,
        r.output_tokens,
        r.cache_read_tokens,
        r.cache_creation_tokens,
        r.duration_ms,
        r.speed,
        r.effort,
        r.query_source,
        r.agent_name,
        r.ts,
    ])?;
    Ok(())
}

/// Merge one prompt turn. Length and command name only — never the text.
pub fn upsert_prompt(conn: &Connection, p: &Prompt) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO prompt (prompt_id, session_id, ts, prompt_length, command_name, command_source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (prompt_id) DO UPDATE SET
             session_id    = COALESCE(excluded.session_id,    prompt.session_id),
             ts            = COALESCE(excluded.ts,            prompt.ts),
             prompt_length = COALESCE(excluded.prompt_length, prompt.prompt_length),
             command_name  = COALESCE(excluded.command_name,  prompt.command_name)",
    )?
    .execute(params![
        p.prompt_id,
        p.session_id,
        p.ts,
        p.prompt_length,
        p.command_name,
        p.command_source
    ])?;
    Ok(())
}

/// Record a mid-session permission mode change.
pub fn insert_permission_mode_change(conn: &Connection, c: &PermissionModeChange) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO permission_mode_change (session_id, from_mode, to_mode, trigger, ts)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![
        c.session_id,
        c.from_mode,
        c.to_mode,
        c.trigger,
        c.ts
    ])?;
    Ok(())
}

/// Turns stored evidence back into projection rows.
///
/// Implemented by the lane crates, which own the parsing. `toolog-core` owns
/// only the mechanism, so it never needs to know a transcript's shape.
pub trait Projector {
    /// Project one stored record. Unknown records should be skipped, not
    /// rejected — new Claude Code record types appear constantly.
    fn project(&mut self, conn: &Connection, event: &RawEvent) -> Result<()>;
}

/// What a re-projection did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReprojectStats {
    /// Evidence records fed to the projector.
    pub scanned: usize,
    /// Tool calls in the rebuilt projection.
    pub tool_calls: i64,
}

/// Delete every projection table, leaving `raw_event` untouched.
pub fn clear_projections(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for table in PROJECTION_TABLES {
        tx.execute(&format!("DELETE FROM {table}"), [])?;
    }
    // External-content FTS keeps its own index; the tool_call delete trigger
    // handles it row by row, but clearing outright is cheaper and leaves no
    // chance of a stale entry.
    tx.execute(
        "INSERT INTO tool_call_fts (tool_call_fts) VALUES ('delete-all')",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

/// Rebuild every projection table from `raw_event`.
///
/// The escape hatch [ADR-0004] exists for: when a parser is fixed or Claude Code
/// changes format, history is re-derived from evidence rather than re-read from
/// files that may have rotated or been deleted.
///
/// [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md
pub fn reproject(
    conn: &Connection,
    lane: Option<Lane>,
    projector: &mut dyn Projector,
) -> Result<ReprojectStats> {
    clear_projections(conn)?;

    let tx = conn.unchecked_transaction()?;
    let mut failed: Option<crate::error::Error> = None;
    let scanned = raw::scan(&tx, lane, &mut |event| {
        if failed.is_some() {
            return;
        }
        if let Err(e) = projector.project(&tx, event) {
            failed = Some(e);
        }
    })?;
    if let Some(e) = failed {
        return Err(e);
    }
    tx.commit()?;

    let tool_calls = conn.query_row("SELECT count(*) FROM tool_call", [], |r| r.get(0))?;
    Ok(ReprojectStats {
        scanned,
        tool_calls,
    })
}
