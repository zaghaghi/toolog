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
use crate::model::ToolCall;
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
        for rule in parse(text)? {
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

/// One rule's hits, with the calls behind them.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Finding {
    pub rule_id: String,
    pub title: String,
    pub explanation: String,
    pub severity: Severity,
    pub scope: Scope,
    /// How many calls the rule matched.
    pub calls: i64,
    /// Distinct sessions those calls fall in.
    pub sessions: i64,
    /// Distinct projects those calls fall in.
    pub projects: Vec<String>,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    /// A handful of the matching calls, newest first, for the drill-through.
    pub examples: Vec<ToolCall>,
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

/// How many examples a finding carries.
const EXAMPLES: u32 = 8;

/// The SQL a rule's conditions compile to, with its bindings.
struct Compiled {
    where_sql: String,
    binds: Vec<Box<dyn ToSql>>,
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
    Compiled { where_sql, binds }
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
            "({}) AND tc.decision IS NOT 'reject' AND EXISTS (
                 SELECT 1 FROM tool_call refused
                 WHERE refused.session_id = tc.session_id
                   AND refused.decision = 'reject'
                   AND refused.tool_name = tc.tool_name
                   AND refused.called_at <= tc.called_at
                   AND refused.tool_use_id <> tc.tool_use_id
                   AND refused.input_summary IS NOT NULL
                   AND tc.input_summary IS NOT NULL
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
    }
    compiled
}

/// Evaluate every rule against the store.
///
/// Ordered by severity, then by how much each rule caught. Dismissed findings
/// keep their place and carry the note, rather than vanishing — a review that
/// hides what was waved through is not a review.
pub fn evaluate(conn: &Connection, rules: &[Rule]) -> Result<Vec<Finding>> {
    let dismissals = dismissals(conn)?;
    let mut findings = Vec::new();

    for rule in rules {
        let compiled = where_for(rule);
        let sql = format!(
            "SELECT count(*), count(DISTINCT tc.session_id),
                    min(tc.called_at), max(tc.called_at)
             FROM tool_call tc
             LEFT JOIN session s ON s.session_id = tc.session_id
             WHERE {}",
            compiled.where_sql
        );
        let refs: Vec<&dyn ToSql> = compiled.binds.iter().map(AsRef::as_ref).collect();
        let (calls, sessions, first_at, last_at): (i64, i64, Option<i64>, Option<i64>) = conn
            .query_row(&sql, refs.as_slice(), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;

        if calls == 0 {
            continue;
        }

        findings.push(Finding {
            rule_id: rule.id.clone(),
            title: rule.title.clone(),
            explanation: rule.explanation.clone(),
            severity: rule.severity,
            scope: rule.scope,
            calls,
            sessions,
            projects: projects_for(conn, &compiled)?,
            first_at,
            last_at,
            examples: examples_for(conn, &compiled)?,
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

fn projects_for(conn: &Connection, compiled: &Compiled) -> Result<Vec<String>> {
    let sql = format!(
        "SELECT DISTINCT s.project_path
         FROM tool_call tc
         LEFT JOIN session s ON s.session_id = tc.session_id
         WHERE ({}) AND s.project_path IS NOT NULL
         ORDER BY s.project_path",
        compiled.where_sql
    );
    let refs: Vec<&dyn ToSql> = compiled.binds.iter().map(AsRef::as_ref).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn examples_for(conn: &Connection, compiled: &Compiled) -> Result<Vec<ToolCall>> {
    let sql = format!(
        "SELECT {}
         FROM tool_call tc
         LEFT JOIN session s ON s.session_id = tc.session_id
         WHERE {}
         ORDER BY tc.called_at DESC, tc.rowid DESC
         LIMIT {EXAMPLES}",
        query::TOOL_CALL_COLUMNS,
        compiled.where_sql
    );
    let refs: Vec<&dyn ToSql> = compiled.binds.iter().map(AsRef::as_ref).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), query::map_tool_call)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// One project's posture: what its calls tripped, worst first (task 6.4).
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct ProjectRisk {
    pub project_path: String,
    pub calls: i64,
    /// Findings by severity, worst first: high, medium, low, info.
    pub by_severity: [i64; 4],
    /// The rules this project tripped, worst first.
    pub rule_ids: Vec<String>,
}

/// Per-project risk, from findings already evaluated.
///
/// Built from the findings rather than by re-querying, so the summary and the
/// list can never disagree about what was found.
pub fn by_project(
    conn: &Connection,
    rules: &[Rule],
    findings: &[Finding],
) -> Result<Vec<ProjectRisk>> {
    let mut out: HashMap<String, ProjectRisk> = HashMap::new();

    for finding in findings.iter().filter(|f| f.dismissed.is_none()) {
        let Some(rule) = rules.iter().find(|r| r.id == finding.rule_id) else {
            continue;
        };
        let compiled = where_for(rule);
        let sql = format!(
            "SELECT s.project_path, count(*)
             FROM tool_call tc
             LEFT JOIN session s ON s.session_id = tc.session_id
             WHERE ({}) AND s.project_path IS NOT NULL
             GROUP BY s.project_path",
            compiled.where_sql
        );
        let refs: Vec<&dyn ToSql> = compiled.binds.iter().map(AsRef::as_ref).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(refs.as_slice(), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;

        for row in rows {
            let (project, calls) = row?;
            let entry = out.entry(project.clone()).or_insert_with(|| ProjectRisk {
                project_path: project,
                calls: 0,
                by_severity: [0; 4],
                rule_ids: Vec::new(),
            });
            entry.calls += calls;
            entry.by_severity[3 - usize::from(finding.severity.rank())] += 1;
            entry.rule_ids.push(finding.rule_id.clone());
        }
    }

    let mut list: Vec<ProjectRisk> = out.into_values().collect();
    list.sort_by(|a, b| {
        b.by_severity
            .cmp(&a.by_severity)
            .then(b.calls.cmp(&a.calls))
    });
    Ok(list)
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
