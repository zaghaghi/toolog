//! The verdict ledger: what a local model said, and what it has not looked at
//! yet (Phase 13, [ADR-0013]).
//!
//! This crate holds no model and links no inference. It holds the *shape* of an
//! answer and the SQL that stores and finds one, for the same reason every other
//! query lives here ([ADR-0003]): all SQL in the workspace is in one place, and
//! the crate that runs llama.cpp reaches data through typed functions.
//!
//! # What is stored, and why that is allowed
//!
//! [ADR-0004] says the projection is a derivation and findings are computed
//! rather than stored. A verdict is neither. It is not reproducible — a
//! different model, quantization or prompt gives a different number — so it
//! cannot be recomputed, and something that cannot be recomputed has to be
//! recorded or lost. It is stored the way [ADR-0012] stores a sighting: keyed on
//! a fingerprint of the question, so old answers stay true statements about what
//! the old question got and a new question starts empty.
//!
//! # What it is not
//!
//! Advisory, and never a rule. Nothing here feeds `rules::evaluate`, changes a
//! severity, or fires a notification. A non-deterministic judge cannot be what
//! an audit trail asserts; it can be a second opinion beside one.
//!
//! [ADR-0003]: ../../../docs/adr/0003-sqlite-as-the-embedded-store.md
//! [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md
//! [ADR-0012]: ../../../docs/adr/0012-store-sightings-not-findings.md
//! [ADR-0013]: ../../../docs/adr/0013-a-verdict-is-stored-not-recomputed.md

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Which question was asked: this model file, and this prompt template.
///
/// Both halves are needed to address an answer. A verdict from a different
/// model, or from the same model under different instructions, is an answer to
/// something else and is never mixed in with these.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Pair {
    /// SHA-256 of the `.gguf` file (task 13.14).
    pub model: String,
    /// SHA-256 of the rendered instructions and the grammar.
    pub prompt: String,
}

impl Pair {
    #[must_use]
    pub fn new(model: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
        }
    }

    /// The form the risk view states in words: `a1b2c3d4e5f6 / 0f1e2d3c4b5a`.
    #[must_use]
    pub fn short(&self) -> String {
        let cut = |s: &String| s.chars().take(12).collect::<String>();
        format!("{} / {}", cut(&self.model), cut(&self.prompt))
    }
}

/// What the model actually said, once the schema has accepted it.
///
/// Defined here rather than in `toolog-llm` because it crosses the boundary in
/// both directions — the inference crate produces one, the store keeps one — and
/// two structurally identical types either side of that line would be a
/// conversion nobody could get wrong only until they did. `toolog-llm` parses
/// into this type and re-exports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Verdict {
    /// One sentence saying what the command does.
    ///
    /// The half of the answer most likely to be worth keeping even when the
    /// score is not — see the phase's stated risks.
    pub intent_summary: String,
    /// One of `toolog_llm::verdict::CATEGORIES`, checked there.
    pub category: String,
    /// 1–5. **Never mixed into a severity column**: a rule's severity is
    /// deterministic and this is not, and a reader must be able to tell at a
    /// glance which is which.
    pub risk_score: i64,
    pub is_destructive: bool,
    pub violates_sandbox: bool,
}

/// One row of `llm_verdict`, as it goes in and comes out.
///
/// `verdict` is `None` for an answer that failed validation, and then `error`
/// says why. The two are never both set and never both absent, which is
/// structural rather than asserted: [`Record::ok`] and [`Record::failed`] are
/// the only ways to build one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Record {
    pub tool_use_id: String,
    /// What the model said, when the schema accepted it.
    pub verdict: Option<Verdict>,
    /// Why the model's answer was rejected. `None` when it was accepted.
    pub error: Option<String>,
    /// When this was recorded, in epoch milliseconds.
    pub at: i64,
    /// How long the model took, in milliseconds.
    pub ms: i64,
}

impl Record {
    /// A verdict the schema accepted.
    #[must_use]
    pub fn ok(tool_use_id: impl Into<String>, verdict: Verdict, at: i64, ms: i64) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            verdict: Some(verdict),
            error: None,
            at,
            ms,
        }
    }

    /// An answer that did not validate (task 13.10). Recorded, not dropped.
    #[must_use]
    pub fn failed(
        tool_use_id: impl Into<String>,
        error: impl Into<String>,
        at: i64,
        ms: i64,
    ) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            verdict: None,
            error: Some(error.into()),
            at,
            ms,
        }
    }

    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

/// Write verdicts. Goes through the single writer ([ADR-0007]) like every other
/// mutation.
///
/// `INSERT OR REPLACE` rather than `INSERT OR IGNORE`: re-running the same
/// question over the same call is a deliberate act — a retry of a failure, or a
/// re-examination — and the newer answer is the one that describes what the
/// model does now. The key means it can only ever replace an answer to the *same*
/// question.
///
/// [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md
pub fn record(conn: &Connection, pair: &Pair, records: &[Record]) -> Result<usize> {
    if records.is_empty() {
        return Ok(0);
    }
    let tx = conn.unchecked_transaction()?;
    let mut written = 0;
    {
        let mut stmt = tx.prepare(
            "INSERT OR REPLACE INTO llm_verdict (
                 tool_use_id, model_fingerprint, prompt_fingerprint,
                 status, error,
                 risk_score, category, intent_summary, is_destructive, violates_sandbox,
                 at, ms)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )?;
        for r in records {
            let v = r.verdict.as_ref();
            written += stmt.execute(params![
                r.tool_use_id,
                pair.model,
                pair.prompt,
                if r.is_ok() { "ok" } else { "failed" },
                r.error,
                v.map(|v| v.risk_score),
                v.map(|v| &v.category),
                v.map(|v| &v.intent_summary),
                v.map(|v| v.is_destructive),
                v.map(|v| v.violates_sandbox),
                r.at,
                r.ms,
            ])?;
        }
    }
    tx.commit()?;
    Ok(written)
}

/// A call waiting to be looked at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Pending {
    pub tool_use_id: String,
    /// The **redacted** projection, never the raw evidence (task 13.11).
    ///
    /// `input_summary` is what every other view reads, and secrets are already
    /// stripped from it. The model is shown the same text a person would see.
    pub command: String,
    pub called_at: Option<i64>,
}

/// The calls this (model, prompt) pair has not answered for, oldest first.
///
/// Three conditions, and each is a decision:
///
/// - **`tool_name = 'Bash'`** — task 13.12. 78% of the corpus and where the
///   destructive vocabulary lives; `Read`/`Edit`/`Write` carry a `target_path`
///   that rules already handle well. Widening is a later decision made with data
///   from this one.
/// - **no `rule_sighting`** — the phase's premise. A call a rule already caught
///   is not unexamined, and running a model over it would be a second opinion
///   nobody asked for. This is the store's own definition of "unmatched", which
///   is what lets the queue be counted without reading the rules file.
/// - **no verdict for this pair** — including a *failed* one, so a call the
///   model cannot answer for is not retried on every pass forever.
///
/// Oldest first (task 13.7), so progress through a store is monotonic and a
/// paused backfill resumes where it stopped rather than where the clock is.
pub fn pending(conn: &Connection, pair: &Pair, limit: u32) -> Result<Vec<Pending>> {
    let mut stmt = conn.prepare(
        "SELECT tc.tool_use_id, tc.input_summary, tc.called_at
           FROM tool_call tc
          WHERE tc.tool_name = 'Bash'
            AND tc.input_summary IS NOT NULL
            AND tc.input_summary <> ''
            AND NOT EXISTS (
                SELECT 1 FROM rule_sighting rs WHERE rs.tool_use_id = tc.tool_use_id)
            AND NOT EXISTS (
                SELECT 1 FROM llm_verdict v
                 WHERE v.tool_use_id = tc.tool_use_id
                   AND v.model_fingerprint = ?
                   AND v.prompt_fingerprint = ?)
          ORDER BY tc.called_at ASC, tc.tool_use_id ASC
          LIMIT ?",
    )?;
    let rows = stmt
        .query_map(params![pair.model, pair.prompt, limit], |row| {
            Ok(Pending {
                tool_use_id: row.get(0)?,
                command: row.get(1)?,
                called_at: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// How far the examination has got, for the risk view and the Status card.
///
/// Every number is over the same (model, prompt) pair, and `eligible` is the
/// denominator all of them share — so a reader can see 412 of 3,618 rather than
/// a percentage with nothing behind it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Progress {
    /// Bash calls no rule has ever matched: the whole population.
    pub eligible: i64,
    /// Verdicts this pair recorded and the schema accepted.
    pub examined: i64,
    /// Answers this pair recorded and the schema rejected (task 13.10).
    pub failed: i64,
    /// `eligible - examined - failed`.
    pub queued: i64,
    /// Median-free, and deliberately so: the mean is what a remaining-time
    /// estimate is built from, and a median would need the whole column.
    pub mean_ms: Option<i64>,
}

pub fn progress(conn: &Connection, pair: &Pair) -> Result<Progress> {
    let eligible: i64 = conn.query_row(
        "SELECT count(*) FROM tool_call tc
          WHERE tc.tool_name = 'Bash'
            AND tc.input_summary IS NOT NULL
            AND tc.input_summary <> ''
            AND NOT EXISTS (
                SELECT 1 FROM rule_sighting rs WHERE rs.tool_use_id = tc.tool_use_id)",
        [],
        |r| r.get(0),
    )?;

    let (examined, failed, mean_ms) = conn.query_row(
        "SELECT
             sum(status = 'ok'),
             sum(status = 'failed'),
             avg(ms)
           FROM llm_verdict
          WHERE model_fingerprint = ? AND prompt_fingerprint = ?",
        params![pair.model, pair.prompt],
        |r| {
            Ok((
                r.get::<_, Option<i64>>(0)?.unwrap_or(0),
                r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                r.get::<_, Option<f64>>(2)?,
            ))
        },
    )?;

    Ok(Progress {
        eligible,
        examined,
        failed,
        // Never negative: a verdict can outlive the call it names (this table is
        // never purged), so `examined` can exceed `eligible` on a store that has
        // been through retention.
        queued: (eligible - examined - failed).max(0),
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a millisecond count rounded to a whole millisecond"
        )]
        mean_ms: mean_ms.map(|m| m.round() as i64),
    })
}

/// One examined call, with what the model said and what it was looking at.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct Scored {
    pub tool_use_id: String,
    pub command: Option<String>,
    pub project_path: Option<String>,
    pub called_at: Option<i64>,
    pub risk_score: i64,
    pub category: String,
    pub intent_summary: String,
    pub is_destructive: bool,
    pub violates_sandbox: bool,
}

/// The highest-scoring commands no rule matched (task 13.16).
///
/// The one list this feature adds to the risk view, and it sits in a section
/// that is explicitly not the rules'.
pub fn top_scoring(
    conn: &Connection,
    pair: &Pair,
    min_score: i64,
    limit: u32,
) -> Result<Vec<Scored>> {
    let mut stmt = conn.prepare(
        "SELECT v.tool_use_id, tc.input_summary, s.project_path, tc.called_at,
                v.risk_score, v.category, v.intent_summary,
                v.is_destructive, v.violates_sandbox
           FROM llm_verdict v
           LEFT JOIN tool_call tc ON tc.tool_use_id = v.tool_use_id
           LEFT JOIN session   s  ON s.session_id  = tc.session_id
          WHERE v.model_fingerprint = ? AND v.prompt_fingerprint = ?
            AND v.status = 'ok'
            AND v.risk_score >= ?
          ORDER BY v.risk_score DESC, tc.called_at DESC
          LIMIT ?",
    )?;
    let rows = stmt
        .query_map(params![pair.model, pair.prompt, min_score, limit], |row| {
            Ok(Scored {
                tool_use_id: row.get(0)?,
                command: row.get(1)?,
                project_path: row.get(2)?,
                called_at: row.get(3)?,
                risk_score: row.get(4)?,
                category: row.get(5)?,
                intent_summary: row.get(6)?,
                is_destructive: row.get(7)?,
                violates_sandbox: row.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// How many examined calls fell at each score, worst first.
///
/// Kept apart from `rules::SeverityTally` on purpose: the risk view must never
/// put an LLM score in a severity column, and giving the two the same type is
/// the first step towards someone doing exactly that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct ScoreTally {
    pub score: i64,
    pub calls: i64,
}

pub fn score_tallies(conn: &Connection, pair: &Pair) -> Result<Vec<ScoreTally>> {
    let mut stmt = conn.prepare(
        "SELECT risk_score, count(*) FROM llm_verdict
          WHERE model_fingerprint = ? AND prompt_fingerprint = ? AND status = 'ok'
          GROUP BY risk_score
          ORDER BY risk_score DESC",
    )?;
    let rows = stmt
        .query_map(params![pair.model, pair.prompt], |row| {
            Ok(ScoreTally {
                score: row.get(0)?,
                calls: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The verdict for one call under one pair, for the detail pane.
pub fn verdict_for(conn: &Connection, pair: &Pair, tool_use_id: &str) -> Result<Option<Record>> {
    let row = conn
        .query_row(
            "SELECT tool_use_id, risk_score, category, intent_summary,
                    is_destructive, violates_sandbox, error, at, ms
               FROM llm_verdict
              WHERE tool_use_id = ? AND model_fingerprint = ? AND prompt_fingerprint = ?",
            params![tool_use_id, pair.model, pair.prompt],
            |row| {
                let risk_score: Option<i64> = row.get(1)?;
                Ok(Record {
                    tool_use_id: row.get(0)?,
                    // The four payload columns are non-null together or null
                    // together — the CHECK on `status` is what keeps that true —
                    // so one of them decides whether there is a verdict at all.
                    verdict: match risk_score {
                        Some(risk_score) => Some(Verdict {
                            category: row.get(2)?,
                            intent_summary: row.get(3)?,
                            risk_score,
                            is_destructive: row.get(4)?,
                            violates_sandbox: row.get(5)?,
                        }),
                        None => None,
                    },
                    error: row.get(6)?,
                    at: row.get(7)?,
                    ms: row.get(8)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// A comparison an `@llm-risk:` token can express: `>=4`, `4`, `<2`.
///
/// A score is a number on a scale, unlike `@risk:high` which is a word from a
/// closed set — so the useful question is "at least this bad", and a filter that
/// could only say "exactly 4" would make the reader type four tokens to ask it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScoreFilter {
    pub op: ScoreOp,
    pub score: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreOp {
    Ge,
    Gt,
    Le,
    Lt,
    Eq,
}

impl ScoreOp {
    pub(crate) fn sql(self) -> &'static str {
        match self {
            Self::Ge => ">=",
            Self::Gt => ">",
            Self::Le => "<=",
            Self::Lt => "<",
            Self::Eq => "=",
        }
    }
}

impl ScoreFilter {
    /// Read `>=4`, `>3`, `<=2`, `<2`, `=5` or a bare `4`.
    ///
    /// Returns `None` for anything else rather than guessing: `@llm-risk:high`
    /// is a reader confusing this with `@risk:`, and answering it with an empty
    /// list would teach them the wrong thing.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (op, rest) = match text.as_bytes() {
            [b'>', b'=', ..] => (ScoreOp::Ge, &text[2..]),
            [b'<', b'=', ..] => (ScoreOp::Le, &text[2..]),
            [b'>', ..] => (ScoreOp::Gt, &text[1..]),
            [b'<', ..] => (ScoreOp::Lt, &text[1..]),
            [b'=', ..] => (ScoreOp::Eq, &text[1..]),
            _ => (ScoreOp::Eq, text),
        };
        let score: i64 = rest.trim().parse().ok()?;
        (1..=5).contains(&score).then_some(Self { op, score })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn pair() -> Pair {
        Pair::new("model-aaa", "prompt-bbb")
    }

    fn said(risk_score: i64, category: &str, summary: &str, destructive: bool) -> Verdict {
        Verdict {
            intent_summary: summary.to_string(),
            category: category.to_string(),
            risk_score,
            is_destructive: destructive,
            violates_sandbox: false,
        }
    }

    fn store() -> Db {
        let db = Db::open_in_memory().expect("open");
        db.conn()
            .execute_batch(
                "INSERT INTO session (session_id, project_path) VALUES ('s1', '/p');
                 INSERT INTO tool_call (tool_use_id, session_id, tool_name, input_summary, called_at)
                 VALUES ('t1', 's1', 'Bash', 'ls -la', 100),
                        ('t2', 's1', 'Bash', 'rm -rf /', 200),
                        ('t3', 's1', 'Bash', 'git status', 300),
                        ('t4', 's1', 'Read', 'src/main.rs', 400),
                        ('t5', 's1', 'Bash', '', 500);
                 INSERT INTO rule_sighting (rule_id, fingerprint, tool_use_id, first_seen)
                 VALUES ('destructive', 'fp', 't2', 250);",
            )
            .expect("seed");
        db
    }

    #[test]
    fn the_queue_is_bash_that_no_rule_matched_oldest_first() {
        let db = store();
        let queued = pending(db.conn(), &pair(), 10).expect("pending");
        let ids: Vec<&str> = queued.iter().map(|p| p.tool_use_id.as_str()).collect();
        assert_eq!(
            ids,
            ["t1", "t3"],
            "t2 has a sighting, t4 is not Bash, t5 has no command — and t1 ran first"
        );
        assert_eq!(queued[0].command, "ls -la");
    }

    #[test]
    fn a_call_this_pair_has_answered_for_leaves_the_queue() {
        let db = store();
        let p = pair();
        record(
            db.conn(),
            &p,
            &[Record::ok(
                "t1",
                said(1, "read", "Lists files.", false),
                1,
                900,
            )],
        )
        .expect("record");

        let ids: Vec<String> = pending(db.conn(), &p, 10)
            .expect("pending")
            .into_iter()
            .map(|p| p.tool_use_id)
            .collect();
        assert_eq!(ids, ["t3"]);
    }

    /// Task 13.10: a failure is recorded, which is also what stops it being
    /// retried forever.
    #[test]
    fn a_failed_verdict_is_stored_and_takes_the_call_out_of_the_queue() {
        let db = store();
        let p = pair();
        record(
            db.conn(),
            &p,
            &[Record::failed(
                "t1",
                "risk_score is 9, and the scale is 1 to 5",
                1,
                800,
            )],
        )
        .expect("record");

        assert!(
            !pending(db.conn(), &p, 10)
                .expect("pending")
                .iter()
                .any(|q| q.tool_use_id == "t1"),
            "a call the model could not answer for is not asked again on the next pass"
        );

        let progress = progress(db.conn(), &p).expect("progress");
        assert_eq!(progress.eligible, 2);
        assert_eq!(progress.examined, 0);
        assert_eq!(progress.failed, 1);
        assert_eq!(progress.queued, 1);

        let stored = verdict_for(db.conn(), &p, "t1")
            .expect("read")
            .expect("row");
        assert!(!stored.is_ok());
        assert!(stored.error.expect("a reason").contains("scale is 1 to 5"));
    }

    /// The property migration 008 exists for: change the question and the old
    /// answers stay addressable while the new question starts empty.
    #[test]
    fn a_different_model_or_prompt_starts_a_fresh_set_of_verdicts() {
        let db = store();
        let first = pair();
        record(
            db.conn(),
            &first,
            &[Record::ok(
                "t1",
                said(2, "read", "Lists files.", false),
                1,
                900,
            )],
        )
        .expect("record");

        for other in [
            Pair::new("model-ccc", "prompt-bbb"),
            Pair::new("model-aaa", "prompt-ccc"),
        ] {
            assert_eq!(
                progress(db.conn(), &other).expect("progress").examined,
                0,
                "{} is a different question and starts empty",
                other.short()
            );
            assert_eq!(
                pending(db.conn(), &other, 10)
                    .map(|p| p.len())
                    .expect("pending"),
                2,
                "and every eligible call is queued for it again"
            );
        }

        assert_eq!(
            verdict_for(db.conn(), &first, "t1")
                .expect("read")
                .expect("row")
                .verdict
                .expect("a verdict")
                .risk_score,
            2,
            "while the original answer stays addressable by its own fingerprints"
        );
    }

    #[test]
    fn re_running_the_same_question_replaces_the_answer_rather_than_erroring() {
        let db = store();
        let p = pair();
        record(db.conn(), &p, &[Record::failed("t1", "no JSON", 1, 100)]).expect("first");
        record(
            db.conn(),
            &p,
            &[Record::ok(
                "t1",
                said(3, "vcs", "Lists files.", false),
                2,
                950,
            )],
        )
        .expect("second");

        let progress = progress(db.conn(), &p).expect("progress");
        assert_eq!((progress.examined, progress.failed), (1, 0));
    }

    #[test]
    fn the_worst_examined_calls_come_back_with_what_they_were() {
        let db = store();
        let p = pair();
        record(
            db.conn(),
            &p,
            &[
                Record::ok("t1", said(1, "read", "Lists files.", false), 1, 900),
                Record::ok("t3", said(4, "vcs", "Rewrites history.", true), 1, 900),
            ],
        )
        .expect("record");

        let worst = top_scoring(db.conn(), &p, 3, 10).expect("top");
        assert_eq!(worst.len(), 1);
        assert_eq!(worst[0].tool_use_id, "t3");
        assert_eq!(worst[0].command.as_deref(), Some("git status"));
        assert_eq!(worst[0].project_path.as_deref(), Some("/p"));
        assert!(worst[0].is_destructive);

        let tallies = score_tallies(db.conn(), &p).expect("tallies");
        assert_eq!(tallies[0], ScoreTally { score: 4, calls: 1 });
        assert_eq!(tallies[1], ScoreTally { score: 1, calls: 1 });
    }

    #[test]
    fn the_summary_index_follows_the_verdict_it_describes() {
        let db = store();
        let p = pair();
        record(
            db.conn(),
            &p,
            &[Record::ok(
                "t1",
                said(1, "read", "Lists the working directory.", false),
                1,
                9,
            )],
        )
        .expect("record");

        let hits: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM llm_verdict_fts WHERE llm_verdict_fts MATCH 'working'",
                [],
                |r| r.get(0),
            )
            .expect("search");
        assert_eq!(hits, 1);

        db.conn()
            .execute("DELETE FROM llm_verdict WHERE tool_use_id = 't1'", [])
            .expect("delete");
        let after: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM llm_verdict_fts", [], |r| r.get(0))
            .expect("count");
        assert_eq!(after, 0, "the index must not outlive the row");
    }

    #[test]
    fn a_failed_verdict_puts_nothing_in_the_search_index() {
        let db = store();
        let p = pair();
        record(db.conn(), &p, &[Record::failed("t1", "no JSON", 1, 100)]).expect("record");
        let rows: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM llm_verdict_fts", [], |r| r.get(0))
            .expect("count");
        assert_eq!(rows, 0);
    }

    #[test]
    fn score_comparisons_are_read_the_way_they_are_typed() {
        use ScoreOp::{Eq, Ge, Gt, Le, Lt};
        for (text, op, score) in [
            (">=4", Ge, 4),
            (">3", Gt, 3),
            ("<=2", Le, 2),
            ("<2", Lt, 2),
            ("=5", Eq, 5),
            ("4", Eq, 4),
            (" >= 4 ", Ge, 4),
        ] {
            assert_eq!(
                ScoreFilter::parse(text),
                Some(ScoreFilter { op, score }),
                "{text}"
            );
        }
        for bad in ["high", "", ">", "6", "0", "-1", ">=x", "4.5"] {
            assert_eq!(ScoreFilter::parse(bad), None, "{bad} is not a score");
        }
    }

    #[test]
    fn a_pair_is_named_by_both_halves_of_the_question() {
        let p = Pair::new("a".repeat(64), "b".repeat(64));
        assert_eq!(p.short(), "aaaaaaaaaaaa / bbbbbbbbbbbb");
    }
}
