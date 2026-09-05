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

use rusqlite::{Connection, OptionalExtension as _, named_params, params};

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
                              cc_version, entrypoint, agent_name, slug, first_seen, last_seen)
         VALUES (:session_id, :project_path, :transcript_path, :cwd, :git_branch,
                 :cc_version, :entrypoint, :agent_name, :slug, :first_seen, :last_seen)
         ON CONFLICT (session_id) DO UPDATE SET
             project_path    = COALESCE(excluded.project_path,    session.project_path),
             transcript_path = COALESCE(excluded.transcript_path, session.transcript_path),
             cwd             = COALESCE(excluded.cwd,             session.cwd),
             git_branch      = COALESCE(excluded.git_branch,      session.git_branch),
             cc_version      = COALESCE(excluded.cc_version,      session.cc_version),
             entrypoint      = COALESCE(excluded.entrypoint,      session.entrypoint),
             agent_name      = COALESCE(excluded.agent_name,      session.agent_name),
             slug            = COALESCE(excluded.slug,            session.slug),
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
        ":slug": s.slug,
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
             agent_id, agent_name, tool_name, tool_kind, mcp_server, mcp_tool, called_at, completed_at,
             input_json, input_summary, target_path, result_json, result_text, result_size,
             success, permission_mode, provenance)
         VALUES (
             :tool_use_id, :session_id, :prompt_id, :message_uuid, :parent_uuid, :is_sidechain,
             :agent_id, :agent_name, :tool_name, :tool_kind, :mcp_server, :mcp_tool, :called_at, :completed_at,
             :input_json, :input_summary, :target_path, :result_json, :result_text, :result_size,
             :success, :permission_mode, :bit)
         ON CONFLICT (tool_use_id) DO UPDATE SET
             session_id    = COALESCE(excluded.session_id,    tool_call.session_id),
             prompt_id     = COALESCE(excluded.prompt_id,     tool_call.prompt_id),
             message_uuid  = COALESCE(excluded.message_uuid,  tool_call.message_uuid),
             parent_uuid   = COALESCE(excluded.parent_uuid,   tool_call.parent_uuid),
             is_sidechain  = COALESCE(excluded.is_sidechain,  tool_call.is_sidechain),
             agent_id      = COALESCE(excluded.agent_id,      tool_call.agent_id),
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
             permission_mode = COALESCE(excluded.permission_mode, tool_call.permission_mode),
             provenance    = tool_call.provenance | :bit",
    )?
    .execute(named_params! {
        ":tool_use_id": tool_use_id,
        ":session_id": f.session_id,
        ":prompt_id": f.prompt_id,
        ":message_uuid": f.message_uuid,
        ":parent_uuid": f.parent_uuid,
        ":is_sidechain": f.is_sidechain,
        ":agent_id": f.agent_id,
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
        ":permission_mode": f.permission_mode,
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
             decision_source, success, input_json, input_summary,
             target_path, provenance)
         VALUES (
             :tool_use_id, :session_id, :prompt_id, :message_uuid, :tool_name, :tool_kind,
             :mcp_server, :mcp_tool, :called_at, :duration_ms, :error_type, :decision,
             :decision_source, :success, :attempted_input_json,
             :attempted_input_summary, :attempted_target_path, :bit)
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
             success         = COALESCE(excluded.success,         tool_call.success),
             -- Existing-wins, the reverse of every field above: a transcript's
             -- untruncated input must never be displaced by OTEL's 512-character
             -- copy. This only fills a hole, which is the rejected-call case.
             input_json      = COALESCE(tool_call.input_json,      excluded.input_json),
             input_summary   = COALESCE(tool_call.input_summary,   excluded.input_summary),
             target_path     = COALESCE(tool_call.target_path,     excluded.target_path),
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
        ":success": f.success,
        ":attempted_input_json": f.attempted_input_json,
        ":attempted_input_summary": f.attempted_input_summary,
        ":attempted_target_path": f.attempted_target_path,
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

/// Move a session's working directory, overriding whatever was recorded before.
///
/// A `relocated` record can appear anywhere in a transcript — in practice often
/// at the top, before the records carrying the pre-move `cwd`. Ordinary session
/// upserts merge newest-wins, so relocation has to be applied as a terminal fact
/// once the stream has been read, or the old path simply overwrites it.
pub fn relocate_session(conn: &Connection, session_id: &str, cwd: &str) -> Result<()> {
    conn.prepare_cached("UPDATE session SET cwd = ?2, project_path = ?2 WHERE session_id = ?1")?
        .execute(params![session_id, cwd])?;
    Ok(())
}

/// Record a subagent instance's type, filling only calls that lack one.
///
/// The type appears on some of a subagent's records and not others, so it has to
/// be spread across the instance's calls once the stream has been seen.
pub fn set_agent_type(conn: &Connection, agent_id: &str, agent_name: &str) -> Result<()> {
    conn.prepare_cached(
        "UPDATE tool_call SET agent_name = ?2 WHERE agent_id = ?1 AND agent_name IS NULL",
    )?
    .execute(params![agent_id, agent_name])?;
    Ok(())
}

/// Give every call in a subagent instance the type one of its siblings carried.
///
/// The fallback for instances whose spawning `Agent` call was never seen — in a
/// partial backfill, or when the parent session's transcript is gone.
pub fn spread_agent_names(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE tool_call SET agent_name = (
             SELECT peer.agent_name FROM tool_call peer
             WHERE peer.agent_id = tool_call.agent_id AND peer.agent_name IS NOT NULL
             LIMIT 1)
         WHERE agent_id IS NOT NULL AND agent_name IS NULL",
        [],
    )?;
    Ok(())
}

/// Give calls with no recorded mode the one their session was in.
///
/// A subagent's calls live in their own transcript, and that file carries no
/// `permission-mode` record — 271 of 3,013 calls in the owner's store, all of
/// them sidechain. The mode is a property of the session the subagent was
/// spawned inside, so it is filled from the session's own recorded changes.
///
/// **This is an inference, and the only one in the projection.** It fills only
/// nulls, never replaces an observed value, and prefers the latest change at or
/// before the call — falling back to the session's first known mode for calls
/// that precede any timestamped change.
pub fn inherit_permission_modes(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE tool_call SET permission_mode = (
             SELECT c.to_mode FROM permission_mode_change c
             WHERE c.session_id = tool_call.session_id
               AND c.ts IS NOT NULL AND c.ts <= tool_call.called_at
             ORDER BY c.ts DESC, c.id DESC LIMIT 1)
         WHERE permission_mode IS NULL AND session_id IS NOT NULL AND called_at IS NOT NULL",
        [],
    )?;
    conn.execute(
        "UPDATE tool_call SET permission_mode = (
             SELECT c.to_mode FROM permission_mode_change c
             WHERE c.session_id = tool_call.session_id
             ORDER BY c.id LIMIT 1)
         WHERE permission_mode IS NULL AND session_id IS NOT NULL",
        [],
    )?;
    Ok(())
}

/// The mode a session was last known to be in, if any.
///
/// For the live path, where the projector starts with an empty map partway
/// through a session that has been running for an hour: the mode is in the
/// store from an earlier record, and one indexed lookup recovers it. Without
/// it, every call captured after a restart carried no mode until the next
/// prompt turn resynchronised one — which the live view showed as "—" for a
/// session the store could perfectly well account for.
pub fn last_known_mode(conn: &Connection, session_id: &str) -> Result<Option<String>> {
    let from_changes: Option<String> = conn
        .query_row(
            "SELECT to_mode FROM permission_mode_change
             WHERE session_id = ?1 AND to_mode IS NOT NULL
             ORDER BY ts DESC, id DESC LIMIT 1",
            params![session_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    if from_changes.is_some() {
        return Ok(from_changes);
    }
    // A session whose changes were deduplicated away can still have the mode
    // stamped on its calls.
    Ok(conn
        .query_row(
            "SELECT permission_mode FROM tool_call
             WHERE session_id = ?1 AND permission_mode IS NOT NULL
             ORDER BY called_at DESC, rowid DESC LIMIT 1",
            params![session_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten())
}

/// Turns stored evidence back into projection rows.
///
/// Implemented by the lane crates, which own the parsing. `toolog-core` owns
/// only the mechanism, so it never needs to know a transcript's shape.
pub trait Projector {
    /// Project one stored record. Unknown records should be skipped, not
    /// rejected — new Claude Code record types appear constantly.
    fn project(&mut self, conn: &Connection, event: &RawEvent) -> Result<()>;

    /// Called once after the last record, inside the same transaction.
    ///
    /// For facts that can only be settled with the whole stream in hand — a
    /// subagent's type, for instance, appears on only some of its records and
    /// has to be spread across the rest.
    fn finish(&mut self, conn: &Connection) -> Result<()> {
        let _ = conn;
        Ok(())
    }
}

/// Replays each record through several projectors in turn.
///
/// Re-projection clears **every** projection table and rebuilds it, so it must
/// be given a projector for each lane the evidence store holds. Running it with
/// one lane's projector silently deletes the other lane's columns — which is
/// exactly what a `toolog backfill` did before this existed, taking every
/// `decision`, `decision_source` and `duration_ms` with it.
///
/// Each projector guards on `event.lane`, so ordering does not matter.
pub struct Chain<'a> {
    projectors: Vec<&'a mut dyn Projector>,
}

impl std::fmt::Debug for Chain<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Chain")
            .field("projectors", &self.projectors.len())
            .finish()
    }
}

impl<'a> Chain<'a> {
    #[must_use]
    pub fn new(projectors: Vec<&'a mut dyn Projector>) -> Self {
        Self { projectors }
    }
}

impl Projector for Chain<'_> {
    fn project(&mut self, conn: &Connection, event: &RawEvent) -> Result<()> {
        for projector in &mut self.projectors {
            projector.project(conn, event)?;
        }
        Ok(())
    }

    fn finish(&mut self, conn: &Connection) -> Result<()> {
        for projector in &mut self.projectors {
            projector.finish(conn)?;
        }
        Ok(())
    }
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
    projector.finish(&tx)?;
    tx.commit()?;

    let tool_calls = conn.query_row("SELECT count(*) FROM tool_call", [], |r| r.get(0))?;
    Ok(ReprojectStats {
        scanned,
        tool_calls,
    })
}
