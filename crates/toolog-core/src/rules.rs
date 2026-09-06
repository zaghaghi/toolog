//! Risk rules: what got approved, and how (task 6.1).
//!
//! **Rules are data, not code.** A rule is a row of TOML naming a fixed
//! vocabulary of conditions, so a new rule ships without a release and a user
//! can add their own by editing a file. The vocabulary is deliberately small
//! and typed: the engine compiles it to bound SQL, so a rules file can express
//! a question but never a query.
//!
//! Findings are **computed, never stored**. They are a projection of the store
//! under the current rules, exactly as [ADR-0004] treats every other derived
//! table — change a rule and the findings change with it, with no stale rows to
//! reconcile. The one thing that *is* stored is a dismissal, because that is a
//! judgement a person made and re-running the rules must not discard it.
//!
//! [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md

use std::collections::HashMap;

use rusqlite::{Connection, ToSql, params};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{Page, ToolCall};
use crate::query;

/// The rules shipped with the application.
const BUILT_IN: &str = include_str!("rules/default.toml");

/// How much a finding matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(export_to = "unused/")]
pub enum Severity {
    /// Worth knowing, not worth acting on.
    Info,
    /// Worth a look.
    Low,
    /// Worth explaining.
    Medium,
    /// Worth answering for.
    High,
}

impl Severity {
    /// Highest first, which is the order a review is read in.
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Self::High => 3,
            Self::Medium => 2,
            Self::Low => 1,
            Self::Info => 0,
        }
    }
}

/// What a rule looks at.
///
/// Each variant is a different *shape* of question, and adding a shape is a
/// code change. Adding a rule is not — which is the distinction task 6.1 asks
/// for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export_to = "unused/")]
pub enum Scope {
    /// One row of `tool_call` at a time.
    Call,
    /// A session, flagged through the calls that ran inside it.
    Session,
    /// A refusal followed by an accepted call doing the same thing.
    RetryAfterRefusal,
}

/// The conditions a `call` or `session` rule may state.
///
/// Every field is optional and they are `AND`-ed. A list means "any of these".
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Match {
    /// `tool_name` is one of these.
    #[serde(default)]
    pub tools: Vec<String>,
    /// `tool_kind` is one of these — `builtin`, `mcp`, `skill`, `agent`.
    #[serde(default)]
    pub kinds: Vec<String>,
    /// `decision` is one of these.
    #[serde(default)]
    pub decisions: Vec<String>,
    /// `decision_source` is one of these.
    #[serde(default)]
    pub decision_sources: Vec<String>,
    /// `permission_mode` is one of these.
    #[serde(default)]
    pub permission_modes: Vec<String>,
    /// The call failed (`false`) or succeeded (`true`).
    pub success: Option<bool>,
    /// Only subagent calls (`false`) or only main-thread ones (`true`).
    pub main_thread: Option<bool>,
    /// Match the command's **first line** only.
    ///
    /// Measured against the real corpus, and it is not a nicety. A `cat > f
    /// <<'EOF'` heredoc puts its whole body in the command, so a rule looking
    /// for `rm -rf` flagged a call that was *writing documentation about*
    /// `rm -rf`, and a rule looking for `.env` flagged one writing a
    /// `.gitignore` that mentions it. The first line is the command; the rest
    /// is data it carried.
    #[serde(default)]
    pub first_line: bool,
    /// `input_summary` contains one of these, case-insensitively.
    #[serde(default)]
    pub summary_contains: Vec<String>,
    /// `input_summary` matches one of these SQLite `GLOB` patterns.
    ///
    /// `GLOB` rather than a regular expression: it is case-sensitive, it needs
    /// no extra function registered on every connection, and `*rm*-rf*` is a
    /// pattern a user can write correctly the first time.
    #[serde(default)]
    pub summary_glob: Vec<String>,
    /// `target_path` matches one of these `GLOB` patterns.
    #[serde(default)]
    pub path_glob: Vec<String>,
    /// The call wrote somewhere outside its session's working directory.
    #[serde(default)]
    pub outside_cwd: bool,
    /// The session's permission mode changed while it was running.
    #[serde(default)]
    pub mode_changed: bool,
}

/// One rule, as written in the rules file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub title: String,
    /// What the finding means, and why it is worth a look.
    pub explanation: String,
    pub severity: Severity,
    #[serde(default = "default_scope")]
    pub scope: Scope,
    #[serde(default)]
    pub r#match: Match,
    /// Whether this rule came from the user's file rather than the built-in set.
    ///
    /// Not read from the TOML — set by [`load`] — because it is a fact about
    /// where a rule was found, not something a rules file gets to claim about
    /// itself. The panel says it (task 11.12), because "a built-in you have
    /// replaced" and "a built-in" are different things to be looking at.
    #[serde(skip)]
    pub from_user: bool,
}

fn default_scope() -> Scope {
    Scope::Call
}

#[derive(Debug, Deserialize)]
struct RuleFile {
    #[serde(default)]
    rule: Vec<Rule>,
}

/// Every rule in force, built-in and user-supplied.
///
/// A user rule with the same `id` as a built-in **replaces** it, which is how a
/// rule is switched off or retuned without editing a file the application owns.
pub fn load(user: Option<&str>) -> Result<Vec<Rule>> {
    let mut rules = parse(BUILT_IN)?;
    if let Some(text) = user {
        for mut rule in parse(text)? {
            rule.from_user = true;
            match rules.iter_mut().find(|r| r.id == rule.id) {
                Some(existing) => *existing = rule,
                None => rules.push(rule),
            }
        }
    }
    Ok(rules)
}

fn parse(text: &str) -> Result<Vec<Rule>> {
    let file: RuleFile = toml::from_str(text).map_err(|e| Error::Rules(e.to_string()))?;
    Ok(file.rule)
}

/// One rule and what it found — including a rule that found nothing.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Finding {
    pub rule_id: String,
    pub title: String,
    pub explanation: String,
    pub severity: Severity,
    pub scope: Scope,
    /// What the rule looks for, in words (task 11.12).
    ///
    /// Rendered from [`Match`] by [`describe`], never written by hand: a
    /// description a person maintains beside a rule is a description that
    /// eventually describes a different rule.
    pub conditions: Vec<String>,
    /// Whether the user's rules file supplied or replaced this rule.
    pub from_user: bool,
    /// How many calls the rule matched. **Zero is a result**, not a reason to
    /// be left out (task 11.11).
    pub calls: i64,
    /// Distinct sessions those calls fall in.
    pub sessions: i64,
    /// Distinct projects those calls fall in.
    pub projects: Vec<String>,
    /// Matched calls whose session the store never learned a project for.
    ///
    /// The number that used to vanish: [`reconcile`]'s table dropped these and
    /// the summary counted them, which is half of why the two disagreed (task
    /// 11.8). Carried here so the "no project recorded" row can name the rules
    /// behind it without a second pass over the store.
    pub unattributed_calls: i64,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    /// Set when someone has dismissed this rule, with what they said.
    pub dismissed: Option<Dismissal>,
}

/// A judgement someone made about a rule. Never deletes the calls behind it.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Dismissal {
    pub rule_id: String,
    pub note: String,
    pub at: i64,
}

/// The SQL a rule's conditions compile to, with its bindings.
struct Compiled {
    where_sql: String,
    binds: Vec<Box<dyn ToSql>>,
    /// Whether the statement embedding this needs [`REFUSALS_CTE`] in front.
    needs_refusals: bool,
}

/// The refused calls, gathered once instead of re-found per candidate row.
///
/// Measured, on the owner's store: 4,295 calls, of which **three** are
/// refusals. `retry-after-refusal` was 2,125 ms of a 2,314 ms review — 92% of
/// it — because its correlated `EXISTS` let SQLite seek `refused` by
/// `tool_call_tool_name`, so every candidate Bash call re-scanned every earlier
/// Bash call looking for a rejection among them. `MATERIALIZED` pins the shape:
/// find the three rows once, then the `EXISTS` walks three rows rather than
/// thousands. The same query drops to 2.5 ms.
///
/// It is a plain constant rather than a planner hint (`+refused.tool_name`)
/// because a hint says "do not use this index" and leaves the rest to the
/// planner's estimates; this says what is actually true about the data.
const REFUSALS_CTE: &str = "WITH refusals AS MATERIALIZED (
     SELECT session_id, tool_name, called_at, tool_use_id, input_summary
     FROM tool_call
     WHERE decision = 'reject' AND input_summary IS NOT NULL)
";

impl Compiled {
    /// The `WITH` clause a statement using this fragment must carry.
    fn with_sql(&self) -> &'static str {
        if self.needs_refusals {
            REFUSALS_CTE
        } else {
            ""
        }
    }

    fn refs(&self) -> Vec<&dyn ToSql> {
        self.binds.iter().map(AsRef::as_ref).collect()
    }
}

/// `column IN (?, ?, …)`, or nothing when the list is empty.
fn any_of(
    clauses: &mut Vec<String>,
    binds: &mut Vec<Box<dyn ToSql>>,
    column: &str,
    values: &[String],
) {
    if values.is_empty() {
        return;
    }
    let holes = vec!["?"; values.len()].join(", ");
    clauses.push(format!("{column} IN ({holes})"));
    for v in values {
        binds.push(Box::new(v.clone()));
    }
}

/// `col LIKE ?` / `col GLOB ?` across a list, `OR`-ed together.
///
/// `LIKE` gets an explicit `ESCAPE`, without which the backslashes the
/// caller adds are literal characters and `_` stays a single-character
/// wildcard — which quietly made `id_rsa` a pattern that could only match a
/// string containing a backslash, and so match nothing at all.
fn any_pattern(
    clauses: &mut Vec<String>,
    binds: &mut Vec<Box<dyn ToSql>>,
    column: &str,
    operator: &str,
    values: &[String],
    wrap: impl Fn(&str) -> String,
) {
    if values.is_empty() {
        return;
    }
    let escape = if operator == "LIKE" {
        " ESCAPE '\\'"
    } else {
        ""
    };
    let parts: Vec<String> = values
        .iter()
        .map(|_| format!("{column} {operator} ?{escape}"))
        .collect();
    clauses.push(format!("({})", parts.join(" OR ")));
    for v in values {
        binds.push(Box::new(wrap(v)));
    }
}

/// Compile a rule's conditions into bound SQL over `tool_call tc`.
///
/// Values are bound, never interpolated. A rules file is data a user edits, and
/// data a user edits must not be able to become a query.
fn compile(rule: &Rule) -> Compiled {
    let m = &rule.r#match;
    let mut clauses: Vec<String> = Vec::new();
    let mut binds: Vec<Box<dyn ToSql>> = Vec::new();

    any_of(&mut clauses, &mut binds, "tc.tool_name", &m.tools);
    any_of(&mut clauses, &mut binds, "tc.tool_kind", &m.kinds);
    any_of(&mut clauses, &mut binds, "tc.decision", &m.decisions);
    any_of(
        &mut clauses,
        &mut binds,
        "tc.decision_source",
        &m.decision_sources,
    );
    any_of(
        &mut clauses,
        &mut binds,
        "tc.permission_mode",
        &m.permission_modes,
    );

    if let Some(success) = m.success {
        clauses.push("tc.success = ?".to_string());
        binds.push(Box::new(success));
    }
    if let Some(main) = m.main_thread {
        clauses.push(
            if main {
                "tc.agent_id IS NULL"
            } else {
                "tc.agent_id IS NOT NULL"
            }
            .to_string(),
        );
    }

    // `char(10)` is a newline; `instr` returns 0 when there is none, and
    // `substr(x, 1, -1)` would return the empty string, so the whole value is
    // used unless a newline was actually found.
    let summary = if m.first_line {
        "CASE WHEN instr(tc.input_summary, char(10)) > 0
              THEN substr(tc.input_summary, 1, instr(tc.input_summary, char(10)) - 1)
              ELSE tc.input_summary END"
    } else {
        "tc.input_summary"
    };

    // LIKE is case-insensitive for ASCII in SQLite, which is what "contains"
    // should mean for a shell command.
    any_pattern(
        &mut clauses,
        &mut binds,
        summary,
        "LIKE",
        &m.summary_contains,
        |v| format!("%{}%", v.replace('%', "\\%").replace('_', "\\_")),
    );
    any_pattern(
        &mut clauses,
        &mut binds,
        summary,
        "GLOB",
        &m.summary_glob,
        ToString::to_string,
    );
    any_pattern(
        &mut clauses,
        &mut binds,
        "tc.target_path",
        "GLOB",
        &m.path_glob,
        ToString::to_string,
    );

    if m.outside_cwd {
        // A write whose target is not under the session's working directory.
        // Sessions with no recorded cwd cannot be judged, and are not flagged.
        clauses.push(
            "tc.target_path IS NOT NULL AND s.cwd IS NOT NULL
             AND tc.target_path NOT LIKE s.cwd || '%'"
                .to_string(),
        );
    }
    if m.mode_changed {
        clauses.push(
            "(SELECT count(*) FROM permission_mode_change c
              WHERE c.session_id = tc.session_id AND c.from_mode IS NOT NULL) > 0"
                .to_string(),
        );
    }

    let where_sql = if clauses.is_empty() {
        // A rule that states nothing must match nothing. The alternative is a
        // typo in a rules file flagging the entire store.
        "0".to_string()
    } else {
        clauses.join(" AND ")
    };
    Compiled {
        where_sql,
        binds,
        needs_refusals: false,
    }
}

/// The `WHERE` for a rule, including the shapes that are not row predicates.
fn where_for(rule: &Rule) -> Compiled {
    let mut compiled = compile(rule);

    if rule.scope == Scope::Session {
        // A session-scoped rule is about the session, not about each call in
        // it. Without this the mid-session mode change reported 2,432 calls on
        // the owner's store — true, and useless. The session's first call
        // stands for the session, so the count reads as "14 sessions".
        compiled.where_sql = format!(
            "({}) AND tc.tool_use_id = (
                 SELECT first.tool_use_id FROM tool_call first
                 WHERE first.session_id = tc.session_id
                 ORDER BY first.called_at, first.rowid LIMIT 1)",
            compiled.where_sql
        );
    }

    if rule.scope == Scope::RetryAfterRefusal {
        // A call that ran, in a session that refused the same thing earlier.
        // The agent working around a refusal is the pattern that most justifies
        // this tool existing, so it is matched on what was *attempted* rather
        // than on the tool alone.
        // "Doing the same thing" is judged by one command being a prefix of the
        // other: a workaround is usually the refused command narrowed to a
        // subdirectory or extended with a flag. Compared with `substr` and not
        // with `LIKE`, because `%` and `_` are ordinary characters in a shell
        // command and would silently become wildcards. The eight-character
        // floor keeps `ls` from matching `ls -la`.
        compiled.where_sql = format!(
            "({}) AND tc.decision IS NOT 'reject' AND tc.input_summary IS NOT NULL
             AND EXISTS (
                 SELECT 1 FROM refusals refused
                 WHERE refused.session_id = tc.session_id
                   AND refused.tool_name = tc.tool_name
                   AND refused.called_at <= tc.called_at
                   AND refused.tool_use_id <> tc.tool_use_id
                   AND (
                     (length(refused.input_summary) >= 8
                      AND substr(tc.input_summary, 1, length(refused.input_summary))
                          = refused.input_summary)
                     OR
                     (length(tc.input_summary) >= 8
                      AND substr(refused.input_summary, 1, length(tc.input_summary))
                          = tc.input_summary)
                   ))",
            compiled.where_sql
        );
        compiled.needs_refusals = true;
    }
    compiled
}

/// One rule's own numbers, sliced by project.
///
/// Task 11.1: the count, the project list and the per-project posture are the
/// same scan looked at three ways. They used to be three queries plus a fourth
/// in `by_project`, which is both four times the work and four chances for the
/// summary and the list to disagree.
struct RuleRollup {
    /// `project_path` (`None` for a session the store never learned) to calls.
    by_project: Vec<(Option<String>, i64)>,
    calls: i64,
    sessions: i64,
    first_at: Option<i64>,
    last_at: Option<i64>,
}

fn rollup(conn: &Connection, rule: &Rule) -> Result<RuleRollup> {
    let compiled = where_for(rule);
    let sql = format!(
        "{}SELECT s.project_path,
                 count(DISTINCT tc.tool_use_id),
                 count(DISTINCT tc.session_id),
                 min(tc.called_at), max(tc.called_at)
         FROM tool_call tc
         LEFT JOIN session s ON s.session_id = tc.session_id
         WHERE {}
         GROUP BY s.project_path",
        compiled.with_sql(),
        compiled.where_sql
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(compiled.refs().as_slice(), |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, Option<i64>>(4)?,
        ))
    })?;

    let mut out = RuleRollup {
        by_project: Vec::new(),
        calls: 0,
        sessions: 0,
        first_at: None,
        last_at: None,
    };
    for row in rows {
        let (project, calls, sessions, first, last) = row?;
        out.calls += calls;
        // A session belongs to exactly one project, so grouping by project
        // partitions the sessions and the per-group distinct counts add up.
        out.sessions += sessions;
        out.first_at = min_opt(out.first_at, first);
        out.last_at = max_opt(out.last_at, last);
        out.by_project.push((project, calls));
    }
    out.by_project
        .sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(out)
}

fn min_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (x, None) | (None, x) => x,
    }
}

fn max_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (x, None) | (None, x) => x,
    }
}

/// Evaluate every rule against the store.
///
/// Ordered by severity, then by how much each rule caught. Two things this
/// deliberately does **not** do:
///
/// - **It does not skip a rule that matched nothing** (task 11.11). A rule
///   that found nothing is a real result, the same way Phase 6's empty state
///   is — and skipping it was exactly why a reader could not tell which rules
///   exist without opening a file.
/// - **It does not fetch example calls** (task 11.2). Eight rows were built
///   for every rule on every tab activation and read for at most one. The
///   frontend already pages [`calls`]; the first page is now the first eight.
pub fn evaluate(conn: &Connection, rules: &[Rule]) -> Result<Vec<Finding>> {
    let dismissals = dismissals(conn)?;
    let mut findings = Vec::new();

    for rule in rules {
        let roll = rollup(conn, rule)?;
        findings.push(Finding {
            rule_id: rule.id.clone(),
            title: rule.title.clone(),
            explanation: rule.explanation.clone(),
            severity: rule.severity,
            scope: rule.scope,
            conditions: describe(&rule.r#match),
            from_user: rule.from_user,
            calls: roll.calls,
            sessions: roll.sessions,
            projects: roll
                .by_project
                .iter()
                .filter_map(|(p, _)| p.clone())
                .collect(),
            unattributed_calls: roll
                .by_project
                .iter()
                .find(|(p, _)| p.is_none())
                .map_or(0, |(_, n)| *n),
            first_at: roll.first_at,
            last_at: roll.last_at,
            dismissed: dismissals.get(&rule.id).cloned(),
        });
    }

    findings.sort_by(|a, b| {
        b.severity
            .rank()
            .cmp(&a.severity.rank())
            .then(b.calls.cmp(&a.calls))
            .then(a.rule_id.cmp(&b.rule_id))
    });
    Ok(findings)
}

// ---------------------------------------------------------------------------
// Saying what a rule looks for (task 11.12)
// ---------------------------------------------------------------------------

/// One condition per phrase, in the order [`compile`] applies them.
///
/// Rendered here rather than in the window, and rendered from the struct rather
/// than from prose, for one reason: it sits beside `compile`, so a new
/// condition cannot be added to the vocabulary without a phrase for it. A test
/// asserts every field of [`Match`] produces one.
///
/// `first_line` and `outside_cwd` in particular need saying. Neither is a
/// column, and neither is guessable from a rule's title.
#[must_use]
pub fn describe(m: &Match) -> Vec<String> {
    let list = |values: &[String]| values.join(", ");
    let mut out = Vec::new();

    if !m.tools.is_empty() {
        out.push(format!("the tool is {}", list(&m.tools)));
    }
    if !m.kinds.is_empty() {
        out.push(format!("the tool kind is {}", list(&m.kinds)));
    }
    if !m.decisions.is_empty() {
        out.push(format!("the decision was {}", list(&m.decisions)));
    }
    if !m.decision_sources.is_empty() {
        out.push(format!(
            "the decision came from {}",
            list(&m.decision_sources)
        ));
    }
    if !m.permission_modes.is_empty() {
        out.push(format!(
            "the permission mode was {}",
            list(&m.permission_modes)
        ));
    }
    if let Some(success) = m.success {
        out.push(if success {
            "the call succeeded".to_string()
        } else {
            "the call failed".to_string()
        });
    }
    if let Some(main) = m.main_thread {
        out.push(if main {
            "the call was on the main thread".to_string()
        } else {
            "the call was made by a subagent".to_string()
        });
    }

    // Said once, in front of the patterns it changes, rather than repeated in
    // each of them.
    let where_looked = if m.first_line {
        "the command's first line"
    } else {
        "the command"
    };
    if !m.summary_contains.is_empty() {
        out.push(format!(
            "{where_looked} contains any of: {}",
            list(&m.summary_contains)
        ));
    }
    if !m.summary_glob.is_empty() {
        out.push(format!(
            "{where_looked} matches any of: {}",
            list(&m.summary_glob)
        ));
    }
    if !m.path_glob.is_empty() {
        out.push(format!(
            "the target path matches any of: {}",
            list(&m.path_glob)
        ));
    }
    if m.first_line && m.summary_contains.is_empty() && m.summary_glob.is_empty() {
        // `first_line` with nothing to look at changes nothing, and saying so
        // is better than a phrase that quietly went missing.
        out.push("only the command's first line is looked at".to_string());
    }
    if m.outside_cwd {
        out.push("the call wrote outside its session's working directory".to_string());
    }
    if m.mode_changed {
        out.push("the session's permission mode changed while it was running".to_string());
    }

    if out.is_empty() {
        // `compile` turns a rule stating nothing into `WHERE 0`. Saying that
        // out loud is how a typo in a rules file gets noticed.
        out.push("nothing — this rule states no conditions and matches nothing".to_string());
    }
    out
}

/// Every call one rule matched, newest first (task 6.3's drill-through).
///
/// The eight examples on a [`Finding`] are for reading the finding; this is for
/// leaving it. It returns the calls themselves rather than a filter, because a
/// rule's conditions have no equivalent in [`TimelineFilter`] — `outside_cwd`
/// and `first_line` are not columns — and a drill-through that quietly showed
/// a *similar* set of calls would be worse than none.
pub fn calls(conn: &Connection, rule: &Rule, page: Page) -> Result<Vec<ToolCall>> {
    let compiled = where_for(rule);
    let sql = format!(
        "{}SELECT {}
         FROM tool_call tc
         LEFT JOIN session s ON s.session_id = tc.session_id
         WHERE {}
         ORDER BY tc.called_at DESC, tc.rowid DESC
         LIMIT ? OFFSET ?",
        compiled.with_sql(),
        query::TOOL_CALL_COLUMNS,
        compiled.where_sql
    );
    let mut binds: Vec<&dyn ToSql> = compiled.binds.iter().map(AsRef::as_ref).collect();
    let (limit, offset) = (i64::from(page.limit), i64::from(page.offset));
    binds.push(&limit);
    binds.push(&offset);

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(binds.as_slice(), query::map_tool_call)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Whether one call trips one rule (task 6.12's live check).
///
/// The same compiled condition as [`evaluate`], narrowed to a single call, so
/// a notification cannot claim something the review would not. It is one
/// indexed lookup, which is what makes it affordable on the live path — but
/// only the high-severity rules are worth asking about there, and the caller
/// decides that rather than this function.
///
/// A session-scoped rule is about the session's first call, so asking it about
/// a later one correctly answers `false`: the notification for that already
/// fired when the session started.
pub fn matches(conn: &Connection, rule: &Rule, tool_use_id: &str) -> Result<bool> {
    let compiled = where_for(rule);
    let sql = format!(
        "{}SELECT EXISTS (
             SELECT 1 FROM tool_call tc
             LEFT JOIN session s ON s.session_id = tc.session_id
             WHERE ({}) AND tc.tool_use_id = ?)",
        compiled.with_sql(),
        compiled.where_sql
    );
    let mut binds: Vec<&dyn ToSql> = compiled.binds.iter().map(AsRef::as_ref).collect();
    binds.push(&tool_use_id);
    Ok(conn.query_row(&sql, binds.as_slice(), |r| r.get(0))?)
}

// ---------------------------------------------------------------------------
// Reconciliation: one unit for the summary and the table (tasks 11.7–11.10)
// ---------------------------------------------------------------------------

/// One project's posture, counted in the unit the summary uses (task 6.4).
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct ProjectRisk {
    /// `None` is the **no project recorded** row.
    ///
    /// Those calls were dropped from this table and counted in the summary, so
    /// the two could not be made to agree (task 11.8). A session the store
    /// never learned a path for is a real row now, not a rounding error.
    pub project_path: Option<String>,
    /// **Distinct calls flagged** at each severity, worst first: high, medium,
    /// low, info. A call two rules caught is one call.
    pub by_severity: [i64; 4],
    /// The rules this project tripped, worst first.
    pub rule_ids: Vec<String>,
}

/// The four numbers a review opens with, and what they are counted in.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct SeverityTally {
    pub severity: Severity,
    /// Distinct calls flagged by any live rule of this severity.
    ///
    /// The unit the table's column adds up to, exactly (task 11.7). It used to
    /// be a count of *rules*, while the table counted *(rule, project) pairs* —
    /// which is how one rule spanning three projects appeared three times.
    pub calls: i64,
    /// How many live rules of this severity matched anything.
    ///
    /// Kept as the secondary line under the number (task 11.10). Both are worth
    /// having; only one of them can be the total.
    pub rules: i64,
}

/// The whole reconciliation: the summary numbers and the table under them.
///
/// Built together, from one pass per severity, so they cannot disagree. What
/// they still will **not** do is add to a grand total, and that is a property of
/// the question rather than a defect (task 11.9): a call caught by a `high` rule
/// and a `low` rule is one call at each severity, so the four numbers overlap.
/// There is no grand total on the page because there is no honest one.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Reconciled {
    pub totals: Vec<SeverityTally>,
    pub projects: Vec<ProjectRisk>,
}

/// Severities worst first, which is the order a review is read in.
const SEVERITIES: [Severity; 4] = [
    Severity::High,
    Severity::Medium,
    Severity::Low,
    Severity::Info,
];

/// The summary and the per-project table, in one pass per severity.
///
/// Dismissed rules are left out of both: setting a rule aside is a judgement
/// about what still needs answering, and Phase 6 decided the finding keeps its
/// place in the list while dropping out of the posture.
pub fn reconcile(conn: &Connection, rules: &[Rule], findings: &[Finding]) -> Result<Reconciled> {
    let live: Vec<&Rule> = findings
        .iter()
        .filter(|f| f.dismissed.is_none() && f.calls > 0)
        .filter_map(|f| rules.iter().find(|r| r.id == f.rule_id))
        .collect();

    let mut projects: HashMap<Option<String>, ProjectRisk> = HashMap::new();
    let mut totals = Vec::new();

    for (slot, severity) in SEVERITIES.into_iter().enumerate() {
        let of_severity: Vec<&&Rule> = live.iter().filter(|r| r.severity == severity).collect();
        totals.push(SeverityTally {
            severity,
            calls: 0,
            rules: i64::try_from(of_severity.len()).unwrap_or(i64::MAX),
        });
        if of_severity.is_empty() {
            continue;
        }

        // Every rule of this severity in one `OR`, so a call two of them caught
        // is counted once — which is the whole of task 11.7. Counting per rule
        // and summing is exactly the double-count being fixed.
        let mut clauses = Vec::new();
        let mut binds: Vec<Box<dyn ToSql>> = Vec::new();
        let mut needs_refusals = false;
        for rule in &of_severity {
            let compiled = where_for(rule);
            clauses.push(format!("({})", compiled.where_sql));
            binds.extend(compiled.binds);
            needs_refusals |= compiled.needs_refusals;
        }

        let sql = format!(
            "{}SELECT s.project_path, count(DISTINCT tc.tool_use_id)
             FROM tool_call tc
             LEFT JOIN session s ON s.session_id = tc.session_id
             WHERE {}
             GROUP BY s.project_path",
            if needs_refusals { REFUSALS_CTE } else { "" },
            clauses.join(" OR ")
        );
        let refs: Vec<&dyn ToSql> = binds.iter().map(AsRef::as_ref).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(refs.as_slice(), |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?))
        })?;

        for row in rows {
            let (project, calls) = row?;
            // A call belongs to exactly one project group, so the column adds
            // up to the number above it by construction rather than by luck.
            totals[slot].calls += calls;
            let entry = projects
                .entry(project.clone())
                .or_insert_with(|| ProjectRisk {
                    project_path: project,
                    by_severity: [0; 4],
                    rule_ids: Vec::new(),
                });
            entry.by_severity[slot] += calls;
        }
    }

    // Which rules each project tripped. Read off the findings rather than
    // re-queried: `evaluate` already grouped every rule by project, and asking
    // the store again for something it just said is nine more near-full scans
    // per review — the exact waste this phase exists to remove.
    for finding in findings
        .iter()
        .filter(|f| f.dismissed.is_none() && f.calls > 0)
    {
        for project in &finding.projects {
            if let Some(entry) = projects.get_mut(&Some(project.clone())) {
                entry.rule_ids.push(finding.rule_id.clone());
            }
        }
        if finding.unattributed_calls > 0
            && let Some(entry) = projects.get_mut(&None)
        {
            entry.rule_ids.push(finding.rule_id.clone());
        }
    }

    let mut list: Vec<ProjectRisk> = projects.into_values().collect();
    list.sort_by(|a, b| {
        b.by_severity
            .cmp(&a.by_severity)
            // The unattributed row sorts last among equals: it is the one row
            // a reader cannot go and look at.
            .then(a.project_path.is_none().cmp(&b.project_path.is_none()))
            .then(a.project_path.cmp(&b.project_path))
    });
    Ok(Reconciled {
        totals,
        projects: list,
    })
}

// ---------------------------------------------------------------------------
// Dismissals
// ---------------------------------------------------------------------------

/// Record a judgement about a rule. Re-dismissing replaces the note.
pub fn dismiss(conn: &Connection, rule_id: &str, note: &str, at: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO rule_dismissal (rule_id, note, dismissed_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT (rule_id) DO UPDATE SET note = excluded.note,
                                             dismissed_at = excluded.dismissed_at",
        params![rule_id, note, at],
    )?;
    Ok(())
}

/// Undo a dismissal. The calls behind it were never touched either way.
pub fn restore(conn: &Connection, rule_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM rule_dismissal WHERE rule_id = ?1",
        params![rule_id],
    )?;
    Ok(())
}

fn dismissals(conn: &Connection) -> Result<HashMap<String, Dismissal>> {
    let mut stmt = conn.prepare("SELECT rule_id, note, dismissed_at FROM rule_dismissal")?;
    let rows = stmt.query_map([], |r| {
        Ok(Dismissal {
            rule_id: r.get(0)?,
            note: r.get(1)?,
            at: r.get(2)?,
        })
    })?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|d| (d.rule_id.clone(), d))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_rules_parse_and_are_unique() {
        let rules = load(None).expect("built-in rules parse");
        assert!(rules.len() >= 8, "the starter set from task 6.2");

        let mut ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "rule ids must be unique");

        for rule in &rules {
            assert!(!rule.title.is_empty(), "{} has no title", rule.id);
            assert!(
                !rule.explanation.is_empty(),
                "{} does not say why it matters",
                rule.id
            );
        }
    }

    #[test]
    fn a_user_rule_replaces_a_built_in_with_the_same_id() {
        let user = r#"
            [[rule]]
            id = "auto-approved-destructive-bash"
            title = "Mine"
            explanation = "Retuned locally."
            severity = "low"
            [rule.match]
            tools = ["Bash"]
            summary_contains = ["dd if="]
        "#;
        let rules = load(Some(user)).expect("load");
        let mine = rules
            .iter()
            .find(|r| r.id == "auto-approved-destructive-bash")
            .expect("still present");
        assert_eq!(mine.title, "Mine");
        assert_eq!(mine.severity, Severity::Low);
        assert_eq!(
            rules.iter().filter(|r| r.id == mine.id).count(),
            1,
            "replaced, not duplicated"
        );
    }

    #[test]
    fn a_rule_that_states_no_conditions_matches_nothing() {
        let rule = Rule {
            id: "empty".into(),
            title: "Empty".into(),
            explanation: String::new(),
            severity: Severity::Info,
            scope: Scope::Call,
            r#match: Match::default(),
            from_user: false,
        };
        assert_eq!(compile(&rule).where_sql, "0");
    }

    #[test]
    fn values_are_bound_rather_than_interpolated() {
        let rule = Rule {
            id: "x".into(),
            title: "x".into(),
            explanation: String::new(),
            severity: Severity::High,
            scope: Scope::Call,
            r#match: Match {
                tools: vec!["Bash'; DROP TABLE tool_call; --".into()],
                ..Match::default()
            },
            from_user: false,
        };
        let compiled = compile(&rule);
        assert!(compiled.where_sql.contains("tc.tool_name IN (?)"));
        assert!(!compiled.where_sql.contains("DROP"));
        assert_eq!(compiled.binds.len(), 1);
    }

    #[test]
    fn a_malformed_rules_file_is_an_error_not_a_panic() {
        assert!(load(Some("this is not toml [[[")).is_err());
        assert!(
            load(Some("[[rule]]\nid = \"x\"")).is_err(),
            "missing fields"
        );
    }
}
