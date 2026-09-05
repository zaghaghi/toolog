//! Usage analytics (task 6.5): what was run, what it cost, and how much of
//! that the store actually knows.
//!
//! Two tables answer different halves of the question and only one of them is
//! ever complete. `tool_call` is written by both lanes, so call counts, error
//! rates and durations cover the whole corpus. `api_request` is OTLP-only:
//! backfilled history has no cost data and never will, because the transcript
//! does not record it. Every aggregate here therefore carries a [`Coverage`],
//! and task 6.8 requires the UI to render that rather than a zero — "$0.00
//! spent" and "we were not watching" are different statements and the second
//! one is the true one.
//!
//! Days are bucketed in the caller's timezone. A day boundary is a local fact;
//! bucketing in UTC would put an evening's work on tomorrow's bar for anyone
//! east of Greenwich, so [`Period::utc_offset_minutes`] shifts the timestamp
//! before the date is taken and the UI passes its own offset.

use rusqlite::{Connection, ToSql, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::query::ToolUsage;

/// The longest gap between two calls that still counts as working.
///
/// Active time is "wall-clock with a tool call at least this often". A session
/// where someone read the diff for an hour and then accepted it is two active
/// stretches, not one hour of work — the store cannot see the reading, and
/// counting the gap as active would invent an hour that was never observed.
pub const IDLE_CUTOFF_MS: i64 = 5 * 60 * 1000;

/// What slice of the corpus an analytics run covers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Period {
    /// Inclusive lower bound in epoch milliseconds.
    pub since: Option<i64>,
    /// Exclusive upper bound in epoch milliseconds.
    pub until: Option<i64>,
    /// One project, or every project.
    pub project_path: Option<String>,
    /// Minutes east of UTC, for bucketing days the way the reader's calendar
    /// does. `-new Date().getTimezoneOffset()` in the browser.
    pub utc_offset_minutes: i32,
}

impl Period {
    /// The same span, immediately before this one (task 6.7).
    ///
    /// `None` when the window is open-ended: "the period before all of time"
    /// is not a comparison, and a made-up one would be worse than none.
    #[must_use]
    pub fn preceding(&self) -> Option<Period> {
        let (since, until) = (self.since?, self.until?);
        let span = until.checked_sub(since)?;
        Some(Period {
            since: Some(since - span),
            until: Some(since),
            project_path: self.project_path.clone(),
            utc_offset_minutes: self.utc_offset_minutes,
        })
    }
}

/// A window compiled to a `WHERE` fragment over the aliases in scope.
struct Bounds {
    /// Constrains `tc` (`tool_call`), joined to `s` (`session`).
    calls: String,
    /// Constrains `ar` (`api_request`), joined to `s` (`session`).
    requests: String,
    binds: Vec<Box<dyn ToSql>>,
}

/// Build both fragments at once so their bindings stay in one order.
///
/// The `api_request` clauses bind the same values a second time rather than
/// sharing them: a statement takes its parameters positionally, and two
/// fragments that happen to be used in one query would otherwise depend on
/// which came first.
fn bounds(window: &Period) -> Bounds {
    let mut calls = vec!["1 = 1".to_string()];
    let mut requests = vec!["1 = 1".to_string()];
    let mut binds: Vec<Box<dyn ToSql>> = Vec::new();

    if let Some(since) = window.since {
        calls.push("tc.called_at >= ?".to_string());
        requests.push("ar.ts >= ?".to_string());
        binds.push(Box::new(since));
    }
    if let Some(until) = window.until {
        calls.push("tc.called_at < ?".to_string());
        requests.push("ar.ts < ?".to_string());
        binds.push(Box::new(until));
    }
    if let Some(project) = &window.project_path {
        calls.push("s.project_path = ?".to_string());
        requests.push("s.project_path = ?".to_string());
        binds.push(Box::new(project.clone()));
    }

    Bounds {
        calls: calls.join(" AND "),
        requests: requests.join(" AND "),
        binds,
    }
}

impl Bounds {
    fn refs(&self) -> Vec<&dyn ToSql> {
        self.binds.iter().map(AsRef::as_ref).collect()
    }
}

/// `date(…)` over an epoch-millisecond column, shifted into the local day.
fn local_day(column: &str, offset_minutes: i32) -> String {
    format!(
        "date({column} / 1000 + {}, 'unixepoch')",
        offset_minutes * 60
    )
}

// ---------------------------------------------------------------------------
// The shapes
// ---------------------------------------------------------------------------

/// What the calls in a window look like. Both lanes contribute, so this is
/// complete for the whole corpus.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct CallStats {
    pub calls: i64,
    /// Calls the store knows failed. Not `calls - successes`: a call the
    /// transcript never reached has no outcome at all.
    pub failures: i64,
    /// Calls with a recorded outcome, which is what a failure rate divides by.
    pub with_outcome: i64,
    pub refused: i64,
    /// Calls made by a subagent rather than the main thread.
    pub sidechain: i64,
    pub sessions: i64,
    pub projects: i64,
    /// Null until the OTLP lane has timed these calls.
    pub p50_ms: Option<i64>,
    pub p95_ms: Option<i64>,
    /// Wall-clock time with a call at least every [`IDLE_CUTOFF_MS`].
    pub active_ms: i64,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    /// Failures as a fraction of the calls with a recorded outcome. `None`
    /// when none of them has one — which is not an error rate of zero.
    pub error_rate: Option<f64>,
    /// Subagent calls as a fraction of all calls.
    pub sidechain_share: Option<f64>,
}

/// Failures over the calls with a known outcome.
///
/// `None` rather than `0.0` when nothing in the window has an outcome: a store
/// of purely OTLP-witnessed refusals has no error rate, and zero would read as
/// "nothing failed".
fn ratio(part: i64, whole: i64) -> Option<f64> {
    (whole > 0).then(|| {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a ratio for display; these counts are far below 2^53"
        )]
        let r = part as f64 / whole as f64;
        r
    })
}

/// Money and tokens. OTLP only — see [`Coverage`].
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct CostStats {
    pub requests: i64,
    pub cost_usd_micros: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    /// Cached input as a fraction of all input the model was given.
    ///
    /// Cache creation counts as input here because it was billed as input; the
    /// ratio answers "how much of what I sent was already known", which is the
    /// question the number is read for.
    pub cache_hit_ratio: Option<f64>,
    /// Every token the requests were billed for.
    pub total_tokens: i64,
}

/// How much of the window the cost lane actually saw (task 6.8).
///
/// The UI needs all four numbers to say something true. "5 of 41 sessions
/// were captured live" is honest; the same store rendered as a total is not.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Coverage {
    pub sessions: i64,
    /// Sessions with at least one `api_request` row.
    pub sessions_with_cost: i64,
    pub calls: i64,
    /// Calls belonging to a session that has cost data.
    pub calls_with_cost: i64,
    /// Whether any cost was captured in this window at all. The UI needs this
    /// to choose between "$0.00" and "we were not watching".
    pub measured: bool,
    /// Whether every session in the window carries cost data.
    pub complete: bool,
}

/// One bucket of a breakdown: a day, a project, a model or a session.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Bucket {
    /// The value grouped on. `None` is a real group — calls whose session the
    /// store never learned, or requests with no model recorded — and is not
    /// folded into the others.
    pub key: Option<String>,
    /// A second line for the bucket: a session's project, a day's weekday.
    pub label: Option<String>,
    pub calls: i64,
    pub failures: i64,
    pub cost_usd_micros: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub requests: i64,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
}

/// Everything the analytics view opens with.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Analytics {
    pub window: Period,
    pub calls: CallStats,
    pub cost: CostStats,
    pub coverage: Coverage,
    /// One bucket per day in the window that had activity. Gaps are absent
    /// rather than zero-filled; the chart fills them, because only the chart
    /// knows how wide a bar is.
    pub by_day: Vec<Bucket>,
    /// Projects, most expensive first, then busiest — the leaderboard of task
    /// 6.7.
    pub by_project: Vec<Bucket>,
    pub by_model: Vec<Bucket>,
    /// Sessions, newest first.
    pub by_session: Vec<Bucket>,
    /// Per-tool frequency, failures and latency, scoped to the window.
    pub tools: Vec<ToolUsage>,
}

// ---------------------------------------------------------------------------
// The queries
// ---------------------------------------------------------------------------

/// Everything task 6.5 asks for, over one window.
pub fn analytics(conn: &Connection, window: &Period) -> Result<Analytics> {
    let b = bounds(window);
    Ok(Analytics {
        window: window.clone(),
        calls: call_stats(conn, &b)?,
        cost: cost_stats(conn, &b)?,
        coverage: coverage(conn, &b)?,
        by_day: by_day(conn, &b, window.utc_offset_minutes)?,
        by_project: by_project(conn, &b)?,
        by_model: by_model(conn, &b)?,
        by_session: by_session(conn, &b)?,
        tools: tools(conn, &b)?,
    })
}

fn call_stats(conn: &Connection, b: &Bounds) -> Result<CallStats> {
    let sql = format!(
        "SELECT count(*),
                sum(CASE WHEN tc.success = 0 THEN 1 ELSE 0 END),
                sum(CASE WHEN tc.success IS NOT NULL THEN 1 ELSE 0 END),
                sum(CASE WHEN tc.decision = 'reject' THEN 1 ELSE 0 END),
                sum(CASE WHEN tc.agent_id IS NOT NULL THEN 1 ELSE 0 END),
                count(DISTINCT tc.session_id),
                count(DISTINCT s.project_path),
                min(tc.called_at), max(tc.called_at)
         FROM tool_call tc
         LEFT JOIN session s ON s.session_id = tc.session_id
         WHERE {}",
        b.calls
    );
    let mut stats = conn.query_row(&sql, b.refs().as_slice(), |r| {
        Ok(CallStats {
            calls: r.get(0)?,
            failures: r.get::<_, Option<i64>>(1)?.unwrap_or_default(),
            with_outcome: r.get::<_, Option<i64>>(2)?.unwrap_or_default(),
            refused: r.get::<_, Option<i64>>(3)?.unwrap_or_default(),
            sidechain: r.get::<_, Option<i64>>(4)?.unwrap_or_default(),
            sessions: r.get(5)?,
            projects: r.get(6)?,
            p50_ms: None,
            p95_ms: None,
            active_ms: 0,
            first_at: r.get(7)?,
            last_at: r.get(8)?,
            error_rate: None,
            sidechain_share: None,
        })
    })?;

    (stats.p50_ms, stats.p95_ms) = percentiles(conn, b)?;
    stats.active_ms = active_ms(conn, b)?;
    stats.error_rate = ratio(stats.failures, stats.with_outcome);
    stats.sidechain_share = ratio(stats.sidechain, stats.calls);
    Ok(stats)
}

/// Overall p50 and p95 duration, by rank rather than by a percentile function
/// SQLite does not ship. `(n * p + 99) / 100` is a ceiling on integer division.
fn percentiles(conn: &Connection, b: &Bounds) -> Result<(Option<i64>, Option<i64>)> {
    let sql = format!(
        "WITH ranked AS (
             SELECT tc.duration_ms AS d,
                    row_number() OVER (ORDER BY tc.duration_ms) AS rn,
                    count(*)      OVER ()                       AS n
             FROM tool_call tc
             LEFT JOIN session s ON s.session_id = tc.session_id
             WHERE ({}) AND tc.duration_ms IS NOT NULL
         )
         SELECT max(CASE WHEN rn <= (n * 50 + 99) / 100 THEN d END),
                max(CASE WHEN rn <= (n * 95 + 99) / 100 THEN d END)
         FROM ranked",
        b.calls
    );
    Ok(conn.query_row(&sql, b.refs().as_slice(), |r| Ok((r.get(0)?, r.get(1)?)))?)
}

/// Wall-clock time with a call at least every [`IDLE_CUTOFF_MS`].
///
/// Gaps are measured within a session, so two sessions running side by side
/// each contribute their own time — which is the honest answer to "how long
/// was this machine working", and deliberately not the same as elapsed time.
fn active_ms(conn: &Connection, b: &Bounds) -> Result<i64> {
    let sql = format!(
        "WITH gaps AS (
             SELECT tc.called_at - lag(tc.called_at)
                    OVER (PARTITION BY tc.session_id ORDER BY tc.called_at, tc.rowid) AS gap
             FROM tool_call tc
             LEFT JOIN session s ON s.session_id = tc.session_id
             WHERE ({}) AND tc.called_at IS NOT NULL
         )
         SELECT COALESCE(sum(gap), 0) FROM gaps
         WHERE gap IS NOT NULL AND gap >= 0 AND gap <= {IDLE_CUTOFF_MS}",
        b.calls
    );
    Ok(conn.query_row(&sql, b.refs().as_slice(), |r| r.get(0))?)
}

fn cost_stats(conn: &Connection, b: &Bounds) -> Result<CostStats> {
    let sql = format!(
        "SELECT count(*),
                COALESCE(sum(ar.cost_usd_micros), 0),
                COALESCE(sum(ar.input_tokens), 0),
                COALESCE(sum(ar.output_tokens), 0),
                COALESCE(sum(ar.cache_read_tokens), 0),
                COALESCE(sum(ar.cache_creation_tokens), 0)
         FROM api_request ar
         LEFT JOIN session s ON s.session_id = ar.session_id
         WHERE {}",
        b.requests
    );
    let mut cost = conn.query_row(&sql, b.refs().as_slice(), |r| {
        Ok(CostStats {
            requests: r.get(0)?,
            cost_usd_micros: r.get(1)?,
            input_tokens: r.get(2)?,
            output_tokens: r.get(3)?,
            cache_read_tokens: r.get(4)?,
            cache_creation_tokens: r.get(5)?,
            cache_hit_ratio: None,
            total_tokens: 0,
        })
    })?;
    cost.total_tokens = cost.input_tokens
        + cost.output_tokens
        + cost.cache_read_tokens
        + cost.cache_creation_tokens;
    cost.cache_hit_ratio = ratio(
        cost.cache_read_tokens,
        cost.input_tokens + cost.cache_read_tokens + cost.cache_creation_tokens,
    );
    Ok(cost)
}

fn coverage(conn: &Connection, b: &Bounds) -> Result<Coverage> {
    // "Has cost" is a property of the session, not of the window: a session
    // captured live has cost for all of it, and one backfilled has none for any
    // of it. Asking per-window would report a session as uncovered whenever the
    // window happened to miss its requests.
    let sql = format!(
        "SELECT count(*), count(DISTINCT tc.session_id),
                sum(CASE WHEN priced.session_id IS NOT NULL THEN 1 ELSE 0 END),
                count(DISTINCT CASE WHEN priced.session_id IS NOT NULL
                                    THEN tc.session_id END)
         FROM tool_call tc
         LEFT JOIN session s ON s.session_id = tc.session_id
         LEFT JOIN (SELECT DISTINCT session_id FROM api_request
                    WHERE session_id IS NOT NULL) priced
                ON priced.session_id = tc.session_id
         WHERE {}",
        b.calls
    );
    let mut coverage = conn.query_row(&sql, b.refs().as_slice(), |r| {
        Ok(Coverage {
            calls: r.get(0)?,
            sessions: r.get(1)?,
            calls_with_cost: r.get::<_, Option<i64>>(2)?.unwrap_or_default(),
            sessions_with_cost: r.get(3)?,
            measured: false,
            complete: false,
        })
    })?;
    coverage.measured = coverage.sessions_with_cost > 0;
    coverage.complete = coverage.sessions > 0 && coverage.sessions_with_cost == coverage.sessions;
    Ok(coverage)
}

/// Calls and cost share a bucket, and neither table can produce the other's
/// rows, so each breakdown is a full outer join written as a union of keys.
///
/// Each side names its own key expression because the two tables agree on no
/// column: a day comes from `tc.called_at` on one side and `ar.ts` on the
/// other, and a model exists on one side only. `None` means that side has
/// nothing to group by and contributes an empty relation rather than a
/// fabricated key — which is why a model's call count is legitimately zero.
fn breakdown(
    conn: &Connection,
    b: &Bounds,
    call_key: Option<&str>,
    request_key: Option<&str>,
    order: &str,
) -> Result<Vec<Bucket>> {
    let calls_cte = match call_key {
        Some(key) => format!(
            "SELECT {key} AS k,
                    count(*) AS calls,
                    sum(CASE WHEN tc.success = 0 THEN 1 ELSE 0 END) AS failures,
                    min(tc.called_at) AS first_at, max(tc.called_at) AS last_at
             FROM tool_call tc
             LEFT JOIN session s ON s.session_id = tc.session_id
             WHERE {}
             GROUP BY k",
            b.calls
        ),
        None => "SELECT NULL AS k, 0 AS calls, 0 AS failures,
                        NULL AS first_at, NULL AS last_at WHERE 0"
            .to_string(),
    };
    let requests_cte = match request_key {
        Some(key) => format!(
            "SELECT {key} AS k,
                    count(*) AS requests,
                    COALESCE(sum(ar.cost_usd_micros), 0)   AS cost,
                    COALESCE(sum(ar.input_tokens), 0)      AS input,
                    COALESCE(sum(ar.output_tokens), 0)     AS output,
                    COALESCE(sum(ar.cache_read_tokens), 0) AS cached
             FROM api_request ar
             LEFT JOIN session s ON s.session_id = ar.session_id
             WHERE {}
             GROUP BY k",
            b.requests
        ),
        None => "SELECT NULL AS k, 0 AS requests, 0 AS cost,
                        0 AS input, 0 AS output, 0 AS cached WHERE 0"
            .to_string(),
    };

    let sql = format!(
        "WITH c AS ({calls_cte}),
              r AS ({requests_cte}),
              keys AS (SELECT k FROM c UNION SELECT k FROM r)
         SELECT keys.k,
                COALESCE(c.calls, 0), COALESCE(c.failures, 0),
                COALESCE(r.cost, 0), COALESCE(r.input, 0), COALESCE(r.output, 0),
                COALESCE(r.cached, 0), COALESCE(r.requests, 0),
                c.first_at, c.last_at
         FROM keys
         LEFT JOIN c ON c.k IS keys.k
         LEFT JOIN r ON r.k IS keys.k
         ORDER BY {order}"
    );

    // Only the sides that are actually in the statement bind their values, and
    // in the order the two CTEs appear.
    let mut binds: Vec<&dyn ToSql> = Vec::new();
    if call_key.is_some() {
        binds.extend(b.refs());
    }
    if request_key.is_some() {
        binds.extend(b.refs());
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(binds.as_slice(), |r| {
        Ok(Bucket {
            key: r.get(0)?,
            label: None,
            calls: r.get(1)?,
            failures: r.get(2)?,
            cost_usd_micros: r.get(3)?,
            input_tokens: r.get(4)?,
            output_tokens: r.get(5)?,
            cache_read_tokens: r.get(6)?,
            requests: r.get(7)?,
            first_at: r.get(8)?,
            last_at: r.get(9)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn by_day(conn: &Connection, b: &Bounds, offset_minutes: i32) -> Result<Vec<Bucket>> {
    breakdown(
        conn,
        b,
        Some(&local_day("tc.called_at", offset_minutes)),
        Some(&local_day("ar.ts", offset_minutes)),
        "keys.k",
    )
}

fn by_project(conn: &Connection, b: &Bounds) -> Result<Vec<Bucket>> {
    // Cost first: this is the leaderboard task 6.7 asks for, and a project with
    // no cost data has not become cheap, it has become unmeasured — so calls
    // break the tie rather than leaving unpriced projects in arbitrary order.
    breakdown(
        conn,
        b,
        Some("s.project_path"),
        Some("s.project_path"),
        "COALESCE(r.cost, 0) DESC, COALESCE(c.calls, 0) DESC, keys.k",
    )
}

fn by_model(conn: &Connection, b: &Bounds) -> Result<Vec<Bucket>> {
    // `tool_call` has no model column: a tool call is not made by a model, it
    // is made in a turn a model asked for. So this breakdown is requests only,
    // and its call counts are zero because there is no such thing to count.
    breakdown(
        conn,
        b,
        None,
        Some("ar.model"),
        "COALESCE(r.cost, 0) DESC, COALESCE(r.requests, 0) DESC, keys.k",
    )
}

fn by_session(conn: &Connection, b: &Bounds) -> Result<Vec<Bucket>> {
    let mut buckets = breakdown(
        conn,
        b,
        Some("tc.session_id"),
        Some("ar.session_id"),
        "c.last_at IS NULL, c.last_at DESC, keys.k",
    )?;

    // A session id is not a name. The project it ran in is what makes the row
    // readable, and it costs one indexed lookup per bucket.
    let mut stmt = conn.prepare("SELECT project_path FROM session WHERE session_id = ?1")?;
    for bucket in &mut buckets {
        let Some(id) = bucket.key.as_deref() else {
            continue;
        };
        bucket.label = stmt
            .query_row([id], |r| r.get::<_, Option<String>>(0))
            .unwrap_or(None);
    }
    Ok(buckets)
}

fn tools(conn: &Connection, b: &Bounds) -> Result<Vec<ToolUsage>> {
    let sql = format!(
        "WITH scoped AS (
             SELECT tc.tool_name, tc.duration_ms, tc.success
             FROM tool_call tc
             LEFT JOIN session s ON s.session_id = tc.session_id
             WHERE ({}) AND tc.tool_name IS NOT NULL
         ),
         ranked AS (
             SELECT tool_name, duration_ms,
                    row_number() OVER (PARTITION BY tool_name ORDER BY duration_ms) AS rn,
                    count(*)     OVER (PARTITION BY tool_name)                      AS n
             FROM scoped WHERE duration_ms IS NOT NULL
         ),
         pct AS (
             SELECT tool_name,
                    max(CASE WHEN rn <= (n * 50 + 99) / 100 THEN duration_ms END) AS p50,
                    max(CASE WHEN rn <= (n * 95 + 99) / 100 THEN duration_ms END) AS p95
             FROM ranked GROUP BY tool_name
         )
         SELECT scoped.tool_name,
                count(*),
                sum(CASE WHEN scoped.success = 0 THEN 1 ELSE 0 END),
                pct.p50, pct.p95
         FROM scoped
         LEFT JOIN pct ON pct.tool_name = scoped.tool_name
         GROUP BY scoped.tool_name
         ORDER BY count(*) DESC, scoped.tool_name",
        b.calls
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(b.refs().as_slice(), |row| {
        Ok(ToolUsage {
            tool_name: row.get(0)?,
            calls: row.get(1)?,
            failures: row.get::<_, Option<i64>>(2)?.unwrap_or_default(),
            p50_ms: row.get(3)?,
            p95_ms: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

// ---------------------------------------------------------------------------
// Comparison (task 6.7)
// ---------------------------------------------------------------------------

/// The headline numbers of one window, for putting two side by side.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Headline {
    pub calls: i64,
    pub failures: i64,
    pub sessions: i64,
    pub active_ms: i64,
    pub cost_usd_micros: i64,
    pub tokens: i64,
    /// Sessions in this window that carry cost data, so a fall in spend can be
    /// told apart from a fall in coverage.
    pub sessions_with_cost: i64,
}

/// Two windows, and the periods they cover.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Comparison {
    pub current: Headline,
    pub current_window: Period,
    /// `None` when the current window is open-ended and has no "before".
    pub previous: Option<Headline>,
    pub previous_window: Option<Period>,
}

/// One window's headline figures.
pub fn headline(conn: &Connection, window: &Period) -> Result<Headline> {
    let b = bounds(window);
    let calls = call_stats(conn, &b)?;
    let cost = cost_stats(conn, &b)?;
    let coverage = coverage(conn, &b)?;
    Ok(Headline {
        calls: calls.calls,
        failures: calls.failures,
        sessions: calls.sessions,
        active_ms: calls.active_ms,
        cost_usd_micros: cost.cost_usd_micros,
        tokens: cost.total_tokens,
        sessions_with_cost: coverage.sessions_with_cost,
    })
}

/// A window against the equally long period before it (task 6.7).
pub fn compare(conn: &Connection, window: &Period) -> Result<Comparison> {
    let previous_window = window.preceding();
    let previous = match &previous_window {
        Some(w) => Some(headline(conn, w)?),
        None => None,
    };
    Ok(Comparison {
        current: headline(conn, window)?,
        current_window: window.clone(),
        previous,
        previous_window,
    })
}

// ---------------------------------------------------------------------------
// Live monitoring (tasks 6.10 and 6.11)
// ---------------------------------------------------------------------------

/// A session with recent activity, and what it is doing.
///
/// "Recent" is the caller's window, not a state flag on the row: nothing in
/// the store records that a session *ended*. Claude Code does not announce it,
/// and a session with no call for ten minutes may be one someone is reading a
/// diff in. So this reports when the last call was and lets the UI say
/// "active" or "idle" from a threshold it can explain.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct LiveSession {
    pub session_id: String,
    pub project_path: Option<String>,
    pub git_branch: Option<String>,
    /// The most recent call's tool — "what it is doing right now".
    pub current_tool: Option<String>,
    /// Whether that call has an outcome yet. `None` means it is still running,
    /// or that no lane has reported one.
    pub current_success: Option<bool>,
    pub last_call_at: Option<i64>,
    pub first_call_at: Option<i64>,
    pub calls: i64,
    pub failures: i64,
    pub refused: i64,
    /// Spend so far. `0` with `priced = false` means "not being watched", which
    /// is not the same as free.
    pub cost_usd_micros: i64,
    pub priced: bool,
    pub permission_mode: Option<String>,
    /// Calls per minute over the last twelve minutes, oldest first — the
    /// sparkline on the lane.
    pub recent: Vec<i64>,
}

/// How many one-minute buckets a lane's sparkline carries.
const RECENT_BUCKETS: i64 = 12;

/// Sessions with a call at or after `since`, most recently active first.
pub fn live_sessions(conn: &Connection, since: i64, now: i64) -> Result<Vec<LiveSession>> {
    let mut stmt = conn.prepare(
        "SELECT tc.session_id, s.project_path, s.git_branch,
                count(*),
                sum(CASE WHEN tc.success = 0 THEN 1 ELSE 0 END),
                sum(CASE WHEN tc.decision = 'reject' THEN 1 ELSE 0 END),
                min(tc.called_at), max(tc.called_at),
                COALESCE((SELECT sum(ar.cost_usd_micros) FROM api_request ar
                          WHERE ar.session_id = tc.session_id), 0),
                EXISTS (SELECT 1 FROM api_request ar
                        WHERE ar.session_id = tc.session_id)
         FROM tool_call tc
         LEFT JOIN session s ON s.session_id = tc.session_id
         WHERE tc.called_at >= ?1 AND tc.session_id IS NOT NULL
         GROUP BY tc.session_id
         ORDER BY max(tc.called_at) DESC",
    )?;

    let rows = stmt.query_map(params![since], |r| {
        Ok(LiveSession {
            session_id: r.get(0)?,
            project_path: r.get(1)?,
            git_branch: r.get(2)?,
            calls: r.get(3)?,
            failures: r.get::<_, Option<i64>>(4)?.unwrap_or_default(),
            refused: r.get::<_, Option<i64>>(5)?.unwrap_or_default(),
            first_call_at: r.get(6)?,
            last_call_at: r.get(7)?,
            cost_usd_micros: r.get(8)?,
            priced: r.get(9)?,
            current_tool: None,
            current_success: None,
            permission_mode: None,
            recent: Vec::new(),
        })
    })?;
    let mut sessions = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    // The newest call in each session, for "what it is doing". A second
    // statement rather than a window function in the group-by above, which
    // would need the whole session's rows in the sorter to pick one of them.
    let mut latest = conn.prepare(
        "SELECT tool_name, success, permission_mode FROM tool_call
         WHERE session_id = ?1
         ORDER BY called_at DESC, rowid DESC LIMIT 1",
    )?;
    let mut buckets = conn.prepare(
        "SELECT (?2 - called_at) / 60000 AS ago, count(*)
         FROM tool_call
         WHERE session_id = ?1 AND called_at > ?2 - ?3
         GROUP BY ago",
    )?;

    for session in &mut sessions {
        if let Ok((tool, success, mode)) = latest.query_row([&session.session_id], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<bool>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        }) {
            session.current_tool = tool;
            session.current_success = success;
            session.permission_mode = mode;
        }

        let mut recent = vec![0i64; usize::try_from(RECENT_BUCKETS).unwrap_or(12)];
        let rows = buckets.query_map(
            params![&session.session_id, now, RECENT_BUCKETS * 60_000],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )?;
        for row in rows {
            let (ago, calls) = row?;
            // Oldest first, so the sparkline reads left to right like time.
            let index = RECENT_BUCKETS - 1 - ago;
            if let Ok(i) = usize::try_from(index)
                && let Some(slot) = recent.get_mut(i)
            {
                *slot = calls;
            }
        }
        session.recent = recent;
    }

    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preceding_window_is_the_same_length_immediately_before() {
        let week = Period {
            since: Some(7_000),
            until: Some(14_000),
            project_path: Some("/p".into()),
            utc_offset_minutes: 60,
        };
        let before = week.preceding().expect("bounded");
        assert_eq!(before.since, Some(0));
        assert_eq!(before.until, Some(7_000));
        assert_eq!(before.project_path.as_deref(), Some("/p"));
        assert_eq!(before.utc_offset_minutes, 60);
    }

    #[test]
    fn an_open_ended_window_has_no_preceding_period() {
        assert!(Period::default().preceding().is_none());
        assert!(
            Period {
                since: Some(1),
                ..Period::default()
            }
            .preceding()
            .is_none()
        );
    }

    #[test]
    fn a_ratio_is_none_rather_than_zero_when_there_is_nothing_to_divide() {
        assert!(ratio(0, 0).is_none(), "no denominator means no ratio");
        assert!(ratio(3, 0).is_none());
        assert_eq!(ratio(1, 4), Some(0.25));
    }
}
