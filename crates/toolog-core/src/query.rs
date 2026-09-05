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
///
/// Shared with [`crate::rules`], which selects the same rows for a finding's
/// drill-through.
pub(crate) const TOOL_CALL_COLUMNS: &str = "
    tc.tool_use_id, tc.session_id, tc.prompt_id, tc.message_uuid, tc.parent_uuid,
    tc.is_sidechain, tc.agent_id, tc.agent_name, tc.tool_name, tc.tool_kind, tc.mcp_server,
    tc.mcp_tool, tc.called_at, tc.completed_at, tc.input_json, tc.input_summary,
    tc.target_path, tc.result_json, tc.result_text, tc.result_size, tc.success,
    tc.duration_ms, tc.error_type, tc.decision, tc.decision_source,
    tc.permission_mode, tc.provenance";

pub(crate) fn map_tool_call(row: &Row<'_>) -> rusqlite::Result<ToolCall> {
    map_tool_call_offset(row, 0)
}

/// [`map_tool_call`] when the columns do not start at index 0.
///
/// Kept separate from the columns list above so a query that selects something
/// before them — the `rowid`, a rank — reads the call from the same one mapper.
fn map_tool_call_offset(row: &Row<'_>, base: usize) -> rusqlite::Result<ToolCall> {
    Ok(ToolCall {
        tool_use_id: row.get(base)?,
        session_id: row.get(base + 1)?,
        prompt_id: row.get(base + 2)?,
        message_uuid: row.get(base + 3)?,
        parent_uuid: row.get(base + 4)?,
        is_sidechain: row.get(base + 5)?,
        agent_id: row.get(base + 6)?,
        agent_name: row.get(base + 7)?,
        tool_name: row.get(base + 8)?,
        tool_kind: row.get(base + 9)?,
        mcp_server: row.get(base + 10)?,
        mcp_tool: row.get(base + 11)?,
        called_at: row.get(base + 12)?,
        completed_at: row.get(base + 13)?,
        input_json: row.get(base + 14)?,
        input_summary: row.get(base + 15)?,
        target_path: row.get(base + 16)?,
        result_json: row.get(base + 17)?,
        result_text: row.get(base + 18)?,
        result_size: row.get(base + 19)?,
        success: row.get(base + 20)?,
        duration_ms: row.get(base + 21)?,
        error_type: row.get(base + 22)?,
        decision: row.get(base + 23)?,
        decision_source: row.get(base + 24)?,
        permission_mode: row.get(base + 25)?,
        provenance: row.get(base + 26)?,
    })
}

/// The `FROM` and `WHERE` of a timeline query, with its bindings.
///
/// One builder serves the page, the count and the grouping, so a filter can
/// never mean one thing in the list and another in the scrollbar above it.
struct Selection {
    from: String,
    where_sql: String,
    binds: Vec<Box<dyn ToSql>>,
    /// The sanitized FTS5 expression, when there is one. Kept because the page
    /// query has to bind it a second time for `snippet()`.
    fts: Option<String>,
    /// Whether a full-text term is in play, which decides both the `FROM` shape
    /// and whether a snippet column exists to read.
    searching: bool,
}

/// Build the `FROM`/`WHERE` fragments and bindings for a [`TimelineFilter`].
///
/// Every value is bound, never interpolated. Free text goes through
/// [`fts::build_query`] first: the corpus is mostly shell commands, and `rm
/// -rf` is FTS5 syntax before it is a search.
fn selection(f: &TimelineFilter) -> Selection {
    let mut clauses: Vec<String> = Vec::new();
    let mut binds: Vec<Box<dyn ToSql>> = Vec::new();

    // A text term joins the FTS index and must be bound before the column
    // filters, because it appears first in the statement.
    let fts = f.query.as_deref().and_then(fts::build_query);
    if let Some(query) = fts.clone() {
        clauses.push("tool_call_fts MATCH ?".to_string());
        binds.push(Box::new(query));
    }

    macro_rules! bind {
        ($opt:expr, $sql:expr) => {
            if let Some(v) = $opt.clone() {
                clauses.push($sql.to_string());
                binds.push(Box::new(v));
            }
        };
    }

    bind!(f.session_id, "tc.session_id = ?");
    if f.session_unknown == Some(true) {
        clauses.push("tc.session_id IS NULL".to_string());
    }
    bind!(f.project_path, "s.project_path = ?");
    bind!(f.tool_name, "tc.tool_name = ?");
    bind!(f.since, "tc.called_at >= ?");
    bind!(f.until, "tc.called_at <= ?");
    bind!(f.success, "tc.success = ?");
    bind!(f.is_sidechain, "tc.is_sidechain = ?");
    bind!(f.decision, "tc.decision = ?");
    bind!(f.decision_source, "tc.decision_source = ?");
    bind!(f.permission_mode, "tc.permission_mode = ?");
    bind!(f.agent_id, "tc.agent_id = ?");

    // Subagent calls carry an `agent_id` and main-thread calls never do, so
    // this is the reliable partition — `is_sidechain` is null on any row the
    // transcript lane has not witnessed.
    if let Some(main) = f.main_thread {
        clauses.push(if main {
            "tc.agent_id IS NULL".to_string()
        } else {
            "tc.agent_id IS NOT NULL".to_string()
        });
    }

    bind!(f.provenance, "tc.provenance = ?");

    // The FTS table is joined rather than used as a subquery so `snippet()`
    // can still be called on it. It is not aliased: FTS5 wants its own name on
    // both the `MATCH` and the auxiliary functions.
    let mut from = String::from(
        "tool_call tc
         LEFT JOIN session s ON s.session_id = tc.session_id",
    );
    if fts.is_some() {
        from.push_str("\n         JOIN tool_call_fts ON tool_call_fts.rowid = tc.rowid");
    }

    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };

    Selection {
        from,
        where_sql,
        binds,
        searching: fts.is_some(),
        fts,
    }
}

/// Marks where a match starts inside [`TimelineRow::snippet`].
///
/// A control character, because the corpus is shell commands and file paths:
/// any printable delimiter — `[`, `«`, `**` — eventually turns up inside the
/// very text it is meant to delimit, and the frontend would highlight noise.
pub const MATCH_OPEN: &str = "\u{1}";
/// Marks where a match ends. See [`MATCH_OPEN`].
pub const MATCH_CLOSE: &str = "\u{2}";

/// A timeline row: the call, the session facts the row displays, and — when
/// the page was searched — where the match was found.
///
/// The session columns ride along rather than being looked up per row in the
/// frontend: at 100k rows a second round trip per row is the difference
/// between a list that scrolls and one that stutters.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct TimelineRow {
    pub call: ToolCall,
    pub project_path: Option<String>,
    pub git_branch: Option<String>,
    /// An FTS `snippet()` with the match bracketed. `None` unless the filter
    /// carried a search term — the match may be in `result_text`, which the
    /// row itself never shows.
    pub snippet: Option<String>,
    /// Lines this call added and removed, summed over the files it touched.
    ///
    /// `None` for a call that changed no file. An `Edit` row without its size
    /// is the one row in this list that says nothing about what it did.
    pub lines_added: Option<i64>,
    pub lines_removed: Option<i64>,
}

fn map_timeline_row(row: &Row<'_>) -> rusqlite::Result<TimelineRow> {
    const N: usize = 27;
    Ok(TimelineRow {
        call: map_tool_call(row)?,
        project_path: row.get(N)?,
        git_branch: row.get(N + 1)?,
        snippet: row.get(N + 2)?,
        lines_added: row.get(N + 3)?,
        lines_removed: row.get(N + 4)?,
    })
}

/// The diff size of a row, by indexed lookup rather than a joined aggregate.
///
/// A `GROUP BY` over `file_change` would build the whole table to answer for
/// one page; this is two index seeks per row, and only for the rows drawn.
const DIFF_SIZE_COLUMNS: &str = "
    (SELECT sum(lines_added)   FROM file_change fc WHERE fc.tool_use_id = tc.tool_use_id),
    (SELECT sum(lines_removed) FROM file_change fc WHERE fc.tool_use_id = tc.tool_use_id)";

/// One page of the timeline, newest first, with what the row needs to draw.
///
/// The page is chosen in a subquery that selects nothing but `rowid`, and only
/// the winning rows are decorated. It reads as one query too many, and it is
/// worth measuring before simplifying: ordering by time means sorting every
/// matching row, and a search whose term appears in the whole corpus was
/// carrying 100k fully-built rows through that sort. Sorting 100k integers and
/// building 200 rows is the same answer, several times faster
/// (`cargo run --release -p toolog-core --example measure_timeline`).
pub fn timeline_rows(
    conn: &Connection,
    filter: &TimelineFilter,
    page: Page,
) -> Result<Vec<TimelineRow>> {
    let sel = selection(filter);

    // `snippet()` needs the FTS table joined *and* matched in the same query as
    // the rows it annotates, so the outer query repeats the match. FTS5 will
    // not accept an alias on either side of `MATCH`, which is why both levels
    // name `tool_call_fts` and rely on the subquery's own scope.
    let (snippet, outer_join, outer_match) = if sel.searching {
        (
            "snippet(tool_call_fts, -1, ?, ?, '…', 10)",
            "JOIN tool_call_fts ON tool_call_fts.rowid = tc.rowid",
            "tool_call_fts MATCH ? AND ",
        )
    } else {
        ("NULL", "", "")
    };

    let sql = format!(
        "SELECT {TOOL_CALL_COLUMNS}, s.project_path, s.git_branch, {snippet},
                {DIFF_SIZE_COLUMNS}
         FROM tool_call tc
         LEFT JOIN session s ON s.session_id = tc.session_id
         {outer_join}
         WHERE {outer_match}tc.rowid IN (
             SELECT tc.rowid
             FROM {}{}
             ORDER BY tc.called_at DESC, tc.rowid DESC
             LIMIT ? OFFSET ?
         )
         ORDER BY tc.called_at DESC, tc.rowid DESC",
        sel.from, sel.where_sql
    );

    // SQLite numbers parameters left to right: the snippet delimiters and the
    // outer match come before anything the subquery binds.
    let mut all: Vec<Box<dyn ToSql>> = Vec::new();
    if sel.searching {
        all.push(Box::new(MATCH_OPEN));
        all.push(Box::new(MATCH_CLOSE));
        all.push(Box::new(sel.fts.clone().unwrap_or_default()));
    }
    all.extend(sel.binds);
    all.push(Box::new(i64::from(page.limit)));
    all.push(Box::new(i64::from(page.offset)));
    let refs: Vec<&dyn ToSql> = all.iter().map(AsRef::as_ref).collect();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), map_timeline_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// One page of the timeline, newest first.
///
/// The calls alone, for callers that serialize them — `toolog export` writes
/// [`ToolCall`], not a row decorated for a list that is not there.
pub fn timeline_page(
    conn: &Connection,
    filter: &TimelineFilter,
    page: Page,
) -> Result<Vec<ToolCall>> {
    Ok(timeline_rows(conn, filter, page)?
        .into_iter()
        .map(|r| r.call)
        .collect())
}

/// How many calls match a filter, ignoring paging.
pub fn timeline_count(conn: &Connection, filter: &TimelineFilter) -> Result<i64> {
    let sel = selection(filter);
    let sql = format!("SELECT count(*) FROM {}{}", sel.from, sel.where_sql);
    let refs: Vec<&dyn ToSql> = sel.binds.iter().map(AsRef::as_ref).collect();
    Ok(conn.query_row(&sql, refs.as_slice(), |r| r.get(0))?)
}

/// The subagent instances inside one session, newest first.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct AgentGroup {
    /// The subagent *instance*. Present on every sidechain call.
    pub agent_id: String,
    /// The subagent *type* (`Explore`, `general-purpose`), where it is known.
    pub agent_name: Option<String>,
    pub calls: i64,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
}

/// One session's worth of the current filter, with its subagents.
///
/// `calls` counts everything in the session; `main_thread_calls` excludes the
/// subagent groups, so a grouped list can size itself without a second query
/// per session.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct SessionGroup {
    /// `None` for calls whose session the store never learned — an OTLP event
    /// that arrived before any transcript line named the session.
    pub session_id: Option<String>,
    pub project_path: Option<String>,
    pub git_branch: Option<String>,
    pub slug: Option<String>,
    pub cc_version: Option<String>,
    pub calls: i64,
    pub main_thread_calls: i64,
    pub failures: i64,
    pub refusals: i64,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    /// Sum of `api_request.cost_usd_micros` for the session.
    ///
    /// `None`, never `0`, when the OTLP lane never saw this session: imported
    /// history has no cost data at all, and a zero would read as free.
    pub cost_usd_micros: Option<i64>,
    pub agents: Vec<AgentGroup>,
}

/// The sessions a filter touches, newest activity first.
///
/// The index a grouped, collapsible list is built from: every group's size is
/// known up front, so the list can compute its own height and fetch only the
/// window a session is actually scrolled to.
pub fn timeline_groups(conn: &Connection, filter: &TimelineFilter) -> Result<Vec<SessionGroup>> {
    let sel = selection(filter);
    let sql = format!(
        "SELECT tc.session_id, s.project_path, s.git_branch, s.slug, s.cc_version,
                count(*),
                sum(CASE WHEN tc.agent_id IS NULL THEN 1 ELSE 0 END),
                sum(CASE WHEN tc.success = 0 THEN 1 ELSE 0 END),
                sum(CASE WHEN tc.decision = 'reject' THEN 1 ELSE 0 END),
                min(tc.called_at), max(tc.called_at)
         FROM {}{}
         GROUP BY tc.session_id
         ORDER BY max(tc.called_at) DESC",
        sel.from, sel.where_sql
    );
    let refs: Vec<&dyn ToSql> = sel.binds.iter().map(AsRef::as_ref).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), |row| {
        Ok(SessionGroup {
            session_id: row.get(0)?,
            project_path: row.get(1)?,
            git_branch: row.get(2)?,
            slug: row.get(3)?,
            cc_version: row.get(4)?,
            calls: row.get(5)?,
            main_thread_calls: row.get(6)?,
            failures: row.get(7)?,
            refusals: row.get(8)?,
            first_at: row.get(9)?,
            last_at: row.get(10)?,
            cost_usd_micros: None,
            agents: Vec::new(),
        })
    })?;
    let mut groups = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    // Subagents, in one pass over the same filter rather than a query per
    // session. 271 of 2,334 calls in the planning corpus are sidechain, so
    // these groups are the common case, not a rarity worth deferring.
    let sql = format!(
        "SELECT tc.session_id, tc.agent_id, max(tc.agent_name),
                count(*), min(tc.called_at), max(tc.called_at)
         FROM {}{}{} tc.agent_id IS NOT NULL
         GROUP BY tc.session_id, tc.agent_id
         ORDER BY max(tc.called_at) DESC",
        sel.from,
        sel.where_sql,
        if sel.where_sql.is_empty() {
            " WHERE"
        } else {
            " AND"
        }
    );
    let refs: Vec<&dyn ToSql> = sel.binds.iter().map(AsRef::as_ref).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            AgentGroup {
                agent_id: row.get(1)?,
                agent_name: row.get(2)?,
                calls: row.get(3)?,
                first_at: row.get(4)?,
                last_at: row.get(5)?,
            },
        ))
    })?;
    for row in rows {
        let (session_id, agent) = row?;
        if let Some(g) = groups.iter_mut().find(|g| g.session_id == session_id) {
            g.agents.push(agent);
        }
    }

    // Cost is a property of the session, not of the filtered slice: narrowing
    // to one tool must not make a session look cheaper than it was.
    let mut stmt = conn.prepare(
        "SELECT session_id, sum(cost_usd_micros) FROM api_request
         WHERE session_id IS NOT NULL AND cost_usd_micros IS NOT NULL
         GROUP BY session_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (session_id, cost) = row?;
        if let Some(g) = groups
            .iter_mut()
            .find(|g| g.session_id.as_deref() == Some(session_id.as_str()))
        {
            g.cost_usd_micros = Some(cost);
        }
    }

    Ok(groups)
}

/// The distinct values the filter controls offer.
///
/// Read from the store rather than hard-coded: Claude Code adds tools and
/// permission modes between releases, and a filter listing values that no
/// longer occur — or missing ones that do — is worse than no filter.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Facets {
    pub projects: Vec<String>,
    pub tools: Vec<String>,
    pub decision_sources: Vec<String>,
    pub permission_modes: Vec<String>,
    pub agents: Vec<String>,
}

/// Every value the filter controls can offer, in one round trip.
pub fn facets(conn: &Connection) -> Result<Facets> {
    let distinct = |sql: &str| -> Result<Vec<String>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    };

    Ok(Facets {
        projects: distinct(
            "SELECT DISTINCT project_path FROM session
             WHERE project_path IS NOT NULL ORDER BY project_path",
        )?,
        tools: distinct(
            "SELECT tool_name FROM tool_call WHERE tool_name IS NOT NULL
             GROUP BY tool_name ORDER BY count(*) DESC, tool_name",
        )?,
        decision_sources: distinct(
            "SELECT DISTINCT decision_source FROM tool_call
             WHERE decision_source IS NOT NULL ORDER BY decision_source",
        )?,
        permission_modes: distinct(
            "SELECT DISTINCT permission_mode FROM tool_call
             WHERE permission_mode IS NOT NULL ORDER BY permission_mode",
        )?,
        agents: distinct(
            "SELECT DISTINCT agent_name FROM tool_call
             WHERE agent_name IS NOT NULL ORDER BY agent_name",
        )?,
    })
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

/// Every column of `SELECT` for a session row.
const SESSION_COLUMNS: &str = "
    session_id, project_path, transcript_path, cwd, git_branch, cc_version,
    entrypoint, agent_name, slug, first_seen, last_seen";

fn map_session(row: &Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        session_id: row.get(0)?,
        project_path: row.get(1)?,
        transcript_path: row.get(2)?,
        cwd: row.get(3)?,
        git_branch: row.get(4)?,
        cc_version: row.get(5)?,
        entrypoint: row.get(6)?,
        agent_name: row.get(7)?,
        slug: row.get(8)?,
        first_seen: row.get(9)?,
        last_seen: row.get(10)?,
    })
}

/// Sessions, most recently active first.
pub fn list_sessions(conn: &Connection, page: Page) -> Result<Vec<Session>> {
    let sql = format!(
        "SELECT {SESSION_COLUMNS} FROM session
         ORDER BY last_seen DESC, session_id
         LIMIT ?1 OFFSET ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![page.limit, page.offset], map_session)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// One session by id. The envelope metadata a detail pane shows.
pub fn session(conn: &Connection, session_id: &str) -> Result<Option<Session>> {
    let sql = format!("SELECT {SESSION_COLUMNS} FROM session WHERE session_id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![session_id], map_session)?;
    rows.next().transpose().map_err(Into::into)
}

/// Where a call's evidence sits in the transcript that recorded it.
///
/// Found on demand from `raw_event` rather than stored on `tool_call`: the
/// lookup is scoped to one transcript's rows, and the alternative is a column
/// that would be null for every row already imported.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct SourceRecord {
    /// The transcript this line came from.
    pub source_ref: String,
    /// Byte offset of the line within that file.
    pub source_offset: Option<i64>,
    /// The stored line, verbatim.
    pub body: String,
}

/// The transcript record that first mentioned a call.
///
/// Matched on `message_uuid` where the projection captured one, since that is
/// exact; otherwise on the `tool_use_id`, which appears in the `tool_use` block
/// of the same line. Both are `LIKE` over the stored body, scoped to the
/// transcript lane — this runs once per click, not once per row.
pub fn source_record(conn: &Connection, call: &ToolCall) -> Result<Option<SourceRecord>> {
    let needles = [
        call.message_uuid
            .as_ref()
            .map(|u| format!("%\"uuid\":\"{u}\"%")),
        Some(format!("%{}%", call.tool_use_id)),
    ];

    for needle in needles.into_iter().flatten() {
        let mut stmt = conn.prepare(
            "SELECT source_ref, source_offset, body FROM raw_event
             WHERE lane = 'transcript' AND body LIKE ?1
             ORDER BY id LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![needle], |row| {
            Ok(SourceRecord {
                source_ref: row.get(0)?,
                source_offset: row.get(1)?,
                body: row.get(2)?,
            })
        })?;
        if let Some(found) = rows.next().transpose()? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

/// Full-text search over commands, paths and result text.
///
/// A thin wrapper over the one timeline query, so search and filtering can
/// never disagree. Blank input returns nothing rather than everything: a caller
/// asking to search for whitespace means an empty search box, and the timeline
/// — which treats a blank term as "no constraint" — is the caller that wants
/// every row.
pub fn search(conn: &Connection, input: &str, page: Page) -> Result<Vec<SearchHit>> {
    if fts::build_query(input).is_none() {
        return Ok(Vec::new());
    }
    let filter = TimelineFilter {
        query: Some(input.to_string()),
        ..TimelineFilter::default()
    };
    Ok(timeline_rows(conn, &filter, page)?
        .into_iter()
        .map(|row| SearchHit {
            tool_call: row.call,
            snippet: row.snippet.unwrap_or_default(),
        })
        .collect())
}

/// Corpus-wide totals.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
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
    let mut out = Reconciliation {
        rejected: conn.query_row(
            "SELECT count(*) FROM tool_call WHERE decision = 'reject'",
            [],
            |r| r.get(0),
        )?,
        ..Reconciliation::default()
    };
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

/// What the store already holds, per lane.
///
/// `doctor` reports this so "is it working?" has a numeric answer rather than
/// a green tick.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct IngestSummary {
    /// Distinct transcript files with at least one stored record.
    pub transcript_files: i64,
    pub transcript_records: i64,
    pub otlp_records: i64,
    /// When anything was last stored, in milliseconds since the epoch.
    pub last_ingest_at: Option<i64>,
}

/// Per-lane ingest counts.
pub fn ingest_summary(conn: &Connection) -> Result<IngestSummary> {
    let (transcript_files, transcript_records) = conn.query_row(
        "SELECT count(DISTINCT source_ref), count(*) FROM raw_event WHERE lane = 'transcript'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    Ok(IngestSummary {
        transcript_files,
        transcript_records,
        otlp_records: conn.query_row(
            "SELECT count(*) FROM raw_event WHERE lane = 'otlp'",
            [],
            |r| r.get(0),
        )?,
        last_ingest_at: conn
            .query_row("SELECT max(ingested_at) FROM raw_event", [], |r| r.get(0))?,
    })
}

/// Records stored at or after `since_ms`.
///
/// The tray's "events today" figure. Counted on `raw_event` rather than
/// `tool_call` deliberately: it answers "is capture alive right now?", and
/// every lane writes there first ([ADR-0004]).
///
/// [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md
pub fn events_since(conn: &Connection, since_ms: i64) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT count(*) FROM raw_event WHERE ingested_at >= ?1",
        params![since_ms],
        |r| r.get(0),
    )?)
}

/// Tool calls by rowid, oldest first.
///
/// What the writer's change hook hands over (task 6.9). Unlike a cursor over
/// new rowids, this returns *updates* too — a call the transcript created and
/// OTEL later completed with a duration and a decision is the same row twice,
/// and the live view wants the second one. Callers key on `tool_use_id` for
/// that reason.
pub fn tool_calls_by_rowid(conn: &Connection, rowids: &[i64]) -> Result<Vec<ToolCall>> {
    if rowids.is_empty() {
        return Ok(Vec::new());
    }
    let holes = vec!["?"; rowids.len()].join(", ");
    let sql = format!(
        "SELECT {TOOL_CALL_COLUMNS} FROM tool_call tc
         WHERE tc.rowid IN ({holes})
         ORDER BY tc.rowid"
    );
    let binds: Vec<&dyn ToSql> = rowids.iter().map(|r| r as &dyn ToSql).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(binds.as_slice(), map_tool_call)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
