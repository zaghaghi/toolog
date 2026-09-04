//! The typed query layer — the only place in the workspace that writes SQL.
//!
//! Callers pass filter structs and receive typed rows. Per [ADR-0003] the
//! frontend never reaches the database directly, so this is the whole read
//! surface of the application.
//!
//! [ADR-0003]: ../../../docs/adr/0003-sqlite-as-the-embedded-store.md

use rusqlite::{Connection, Row, ToSql, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::fts;
use crate::model::{
    FileChange, Page, Reconciliation, SearchHit, Session, TimelineFilter, ToolCall, provenance,
};

/// Every `tool_call` column, in the order [`map_tool_call`] expects.
const TOOL_CALL_COLUMNS: &str = "
    tc.tool_use_id, tc.session_id, tc.prompt_id, tc.message_uuid, tc.parent_uuid,
    tc.is_sidechain, tc.agent_name, tc.tool_name, tc.tool_kind, tc.mcp_server,
    tc.mcp_tool, tc.called_at, tc.completed_at, tc.input_json, tc.input_summary,
    tc.target_path, tc.result_json, tc.result_text, tc.result_size, tc.success,
    tc.duration_ms, tc.error_type, tc.decision, tc.decision_source,
    tc.permission_mode, tc.provenance";

fn map_tool_call(row: &Row<'_>) -> rusqlite::Result<ToolCall> {
    Ok(ToolCall {
        tool_use_id: row.get(0)?,
        session_id: row.get(1)?,
        prompt_id: row.get(2)?,
        message_uuid: row.get(3)?,
        parent_uuid: row.get(4)?,
        is_sidechain: row.get(5)?,
        agent_name: row.get(6)?,
        tool_name: row.get(7)?,
        tool_kind: row.get(8)?,
        mcp_server: row.get(9)?,
        mcp_tool: row.get(10)?,
        called_at: row.get(11)?,
        completed_at: row.get(12)?,
        input_json: row.get(13)?,
        input_summary: row.get(14)?,
        target_path: row.get(15)?,
        result_json: row.get(16)?,
        result_text: row.get(17)?,
        result_size: row.get(18)?,
        success: row.get(19)?,
        duration_ms: row.get(20)?,
        error_type: row.get(21)?,
        decision: row.get(22)?,
        decision_source: row.get(23)?,
        permission_mode: row.get(24)?,
        provenance: row.get(25)?,
    })
}

/// Build the `WHERE` fragment and bindings for a [`TimelineFilter`].
///
/// Every value is bound, never interpolated.
fn where_clause(f: &TimelineFilter) -> (String, Vec<Box<dyn ToSql>>) {
    let mut clauses: Vec<&str> = Vec::new();
    let mut binds: Vec<Box<dyn ToSql>> = Vec::new();

    macro_rules! bind {
        ($opt:expr, $sql:expr) => {
            if let Some(v) = $opt.clone() {
                clauses.push($sql);
                binds.push(Box::new(v));
            }
        };
    }

    bind!(f.session_id, "tc.session_id = ?");
    bind!(f.project_path, "s.project_path = ?");
    bind!(f.tool_name, "tc.tool_name = ?");
    bind!(f.since, "tc.called_at >= ?");
    bind!(f.until, "tc.called_at <= ?");
    bind!(f.success, "tc.success = ?");
    bind!(f.is_sidechain, "tc.is_sidechain = ?");
    bind!(f.decision_source, "tc.decision_source = ?");

    if let Some(mask) = f.provenance_mask {
        clauses.push("(tc.provenance & ?) = ?");
        binds.push(Box::new(mask));
        binds.push(Box::new(mask));
    }

    let sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    (sql, binds)
}

/// One page of the timeline, newest first.
pub fn timeline_page(
    conn: &Connection,
    filter: &TimelineFilter,
    page: Page,
) -> Result<Vec<ToolCall>> {
    let (where_sql, binds) = where_clause(filter);
    let sql = format!(
        "SELECT {TOOL_CALL_COLUMNS}
         FROM tool_call tc
         LEFT JOIN session s ON s.session_id = tc.session_id
         {where_sql}
         ORDER BY tc.called_at DESC, tc.rowid DESC
         LIMIT ? OFFSET ?"
    );

    let mut all: Vec<Box<dyn ToSql>> = binds;
    all.push(Box::new(i64::from(page.limit)));
    all.push(Box::new(i64::from(page.offset)));
    let refs: Vec<&dyn ToSql> = all.iter().map(AsRef::as_ref).collect();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), map_tool_call)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// How many calls match a filter, ignoring paging.
pub fn timeline_count(conn: &Connection, filter: &TimelineFilter) -> Result<i64> {
    let (where_sql, binds) = where_clause(filter);
    let sql = format!(
        "SELECT count(*)
         FROM tool_call tc
         LEFT JOIN session s ON s.session_id = tc.session_id
         {where_sql}"
    );
    let refs: Vec<&dyn ToSql> = binds.iter().map(AsRef::as_ref).collect();
    Ok(conn.query_row(&sql, refs.as_slice(), |r| r.get(0))?)
}

/// One tool call by id.
pub fn tool_call_detail(conn: &Connection, tool_use_id: &str) -> Result<Option<ToolCall>> {
    let sql = format!("SELECT {TOOL_CALL_COLUMNS} FROM tool_call tc WHERE tc.tool_use_id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![tool_use_id], map_tool_call)?;
    rows.next().transpose().map_err(Into::into)
}

/// Files touched by one tool call.
pub fn file_changes(conn: &Connection, tool_use_id: &str) -> Result<Vec<FileChange>> {
    let mut stmt = conn.prepare(
        "SELECT tool_use_id, file_path, lines_added, lines_removed, patch_json
         FROM file_change WHERE tool_use_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![tool_use_id], |row| {
        Ok(FileChange {
            tool_use_id: row.get(0)?,
            file_path: row.get(1)?,
            lines_added: row.get(2)?,
            lines_removed: row.get(3)?,
            patch_json: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Sessions, most recently active first.
pub fn list_sessions(conn: &Connection, page: Page) -> Result<Vec<Session>> {
    let mut stmt = conn.prepare(
        "SELECT session_id, project_path, transcript_path, cwd, git_branch, cc_version,
                entrypoint, agent_name, first_seen, last_seen
         FROM session
         ORDER BY last_seen DESC, session_id
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt.query_map(params![page.limit, page.offset], |row| {
        Ok(Session {
            session_id: row.get(0)?,
            project_path: row.get(1)?,
            transcript_path: row.get(2)?,
            cwd: row.get(3)?,
            git_branch: row.get(4)?,
            cc_version: row.get(5)?,
            entrypoint: row.get(6)?,
            agent_name: row.get(7)?,
            first_seen: row.get(8)?,
            last_seen: row.get(9)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Full-text search over commands, paths and result text.
///
/// `input` is free-form text straight from a search box; [`fts::build_query`]
/// makes it safe. Returns an empty result for blank input rather than every row.
pub fn search(conn: &Connection, input: &str, page: Page) -> Result<Vec<SearchHit>> {
    let Some(query) = fts::build_query(input) else {
        return Ok(Vec::new());
    };

    let sql = format!(
        "SELECT {TOOL_CALL_COLUMNS},
                snippet(tool_call_fts, -1, '[', ']', '…', 12)
         FROM tool_call_fts
         JOIN tool_call tc ON tc.rowid = tool_call_fts.rowid
         WHERE tool_call_fts MATCH ?1
         ORDER BY rank
         LIMIT ?2 OFFSET ?3"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![query, page.limit, page.offset], |row| {
        Ok(SearchHit {
            tool_call: map_tool_call(row)?,
            snippet: row.get(26)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Corpus-wide totals.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Totals {
    pub raw_events: i64,
    pub sessions: i64,
    pub tool_calls: i64,
    pub file_changes: i64,
    pub api_requests: i64,
    /// Sum of `cost_usd_micros`. OTLP only — always zero for backfilled history,
    /// which the UI must say rather than render as a real zero.
    pub cost_usd_micros: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub first_call_at: Option<i64>,
    pub last_call_at: Option<i64>,
}

/// Counts and sums across the whole database.
pub fn stats_totals(conn: &Connection) -> Result<Totals> {
    let one = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [], |r| r.get(0))?) };

    let (first, last) = conn.query_row(
        "SELECT min(called_at), max(called_at) FROM tool_call",
        [],
        |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?)),
    )?;

    Ok(Totals {
        raw_events: one("SELECT count(*) FROM raw_event")?,
        sessions: one("SELECT count(*) FROM session")?,
        tool_calls: one("SELECT count(*) FROM tool_call")?,
        file_changes: one("SELECT count(*) FROM file_change")?,
        api_requests: one("SELECT count(*) FROM api_request")?,
        cost_usd_micros: one("SELECT COALESCE(sum(cost_usd_micros), 0) FROM api_request")?,
        input_tokens: one("SELECT COALESCE(sum(input_tokens), 0) FROM api_request")?,
        output_tokens: one("SELECT COALESCE(sum(output_tokens), 0) FROM api_request")?,
        first_call_at: first,
        last_call_at: last,
    })
}

/// Per-tool frequency, failure rate and latency.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolUsage {
    pub tool_name: String,
    pub calls: i64,
    pub failures: i64,
    /// Null until the OTLP lane has seen these calls.
    pub p50_ms: Option<i64>,
    pub p95_ms: Option<i64>,
}

/// Tool usage, most-called first.
pub fn stats_tool_usage(conn: &Connection) -> Result<Vec<ToolUsage>> {
    // Percentiles by row_number rather than a percentile function, which SQLite
    // does not ship. `(n*p + 99) / 100` is a ceiling on integer division.
    let mut stmt = conn.prepare(
        "WITH ranked AS (
             SELECT tool_name, duration_ms,
                    row_number() OVER (PARTITION BY tool_name ORDER BY duration_ms) AS rn,
                    count(*)     OVER (PARTITION BY tool_name)                      AS n
             FROM tool_call
             WHERE duration_ms IS NOT NULL AND tool_name IS NOT NULL
         ),
         pct AS (
             SELECT tool_name,
                    max(CASE WHEN rn <= (n * 50 + 99) / 100 THEN duration_ms END) AS p50,
                    max(CASE WHEN rn <= (n * 95 + 99) / 100 THEN duration_ms END) AS p95
             FROM ranked GROUP BY tool_name
         )
         SELECT tc.tool_name,
                count(*)                                          AS calls,
                sum(CASE WHEN tc.success = 0 THEN 1 ELSE 0 END)    AS failures,
                pct.p50, pct.p95
         FROM tool_call tc
         LEFT JOIN pct ON pct.tool_name = tc.tool_name
         WHERE tc.tool_name IS NOT NULL
         GROUP BY tc.tool_name
         ORDER BY calls DESC, tc.tool_name",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(ToolUsage {
            tool_name: row.get(0)?,
            calls: row.get(1)?,
            failures: row.get(2)?,
            p50_ms: row.get(3)?,
            p95_ms: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Cross-check the two ingestion lanes ([ADR-0009]).
///
/// Divergence is the point, not an error: OTLP-only calls were rejected,
/// transcript-only calls are gaps in collection. Phase 7's `toolog verify`
/// turns this into a per-session report.
///
/// [ADR-0009]: ../../../docs/adr/0009-correlate-on-tool-use-id.md
pub fn reconcile(conn: &Connection) -> Result<Reconciliation> {
    let mut stmt =
        conn.prepare("SELECT provenance, count(*) FROM tool_call GROUP BY provenance")?;
    let mut out = Reconciliation::default();
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;

    for row in rows {
        let (prov, n) = row?;
        let transcript = prov & provenance::TRANSCRIPT != 0;
        let otel = prov & provenance::OTLP != 0;
        match (transcript, otel) {
            (true, true) => out.both += n,
            (true, false) => out.transcript_only += n,
            (false, true) => out.otel_only += n,
            (false, false) => {}
        }
    }
    Ok(out)
}
