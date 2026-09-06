//! What the model said, and whether it is allowed to have said it.
//!
//! The third of task 13.10's mitigations. The grammar already stops the model
//! emitting anything but the schema, so in practice this layer never fires —
//! which is exactly why it is here. A grammar is a claim about a sampler; this
//! is a claim about the bytes that arrived. If the two ever disagree, the answer
//! is recorded as **failed**, with the reason, rather than silently dropped:
//! "the model was asked and could not answer" and "the model was never asked"
//! are different facts, and a backfill that quietly skipped a call would report
//! the second while meaning the first.
//!
//! The parsed type itself lives in [`toolog_core::llm`], because it crosses the
//! boundary in both directions and two identical structs either side of that
//! line is a conversion waiting to be got wrong.

pub use toolog_core::llm::Verdict;

/// The categories a verdict may carry.
///
/// A fixed vocabulary, mirrored in the grammar and in the instructions, and
/// asserted to match both by `prompt.rs`. Free-text categories would make the
/// risk view's grouping meaningless the first time the model invented a synonym.
pub const CATEGORIES: &[&str] = &[
    "read",
    "search",
    "build",
    "test",
    "vcs",
    "package",
    "network",
    "filesystem",
    "process",
    "config",
    "other",
];

/// The lowest and highest a score may be. Both ends are inclusive.
pub const SCORE_RANGE: std::ops::RangeInclusive<i64> = 1..=5;

/// The longest an intent summary may be, in characters.
///
/// The grammar bounds it at 200 too. This is the same number stated where a
/// reader of the schema will see it, and the one that actually runs if the
/// grammar is ever changed and this is not.
pub const SUMMARY_LIMIT: usize = 200;

/// Why an answer was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Invalid {
    #[error("the model emitted no JSON object")]
    NoJson,
    #[error("not valid JSON: {0}")]
    NotJson(String),
    #[error("missing the field `{0}`")]
    Missing(&'static str),
    #[error("`{field}` is not {expected}")]
    WrongType {
        field: &'static str,
        expected: &'static str,
    },
    #[error("risk_score is {0}, and the scale is 1 to 5")]
    ScoreOutOfRange(i64),
    #[error("category `{0}` is not one this schema defines")]
    UnknownCategory(String),
    #[error("the intent summary is empty")]
    EmptySummary,
}

/// Parse and validate one model answer.
///
/// Accepts the object wherever it starts in the text. The grammar means it will
/// be the whole of it, but a build whose grammar failed to load would otherwise
/// produce nothing usable from output that is almost right, and being lenient
/// about the wrapper while being strict about the contents is the correct place
/// for the leniency.
pub fn parse(text: &str) -> Result<Verdict, Invalid> {
    let start = text.find('{').ok_or(Invalid::NoJson)?;
    let end = text.rfind('}').ok_or(Invalid::NoJson)?;
    if end < start {
        return Err(Invalid::NoJson);
    }
    let json: serde_json::Value =
        serde_json::from_str(&text[start..=end]).map_err(|e| Invalid::NotJson(e.to_string()))?;

    let object = json.as_object().ok_or(Invalid::NoJson)?;

    let field = |name: &'static str| object.get(name).ok_or(Invalid::Missing(name));

    let summary = field("intent_summary")?
        .as_str()
        .ok_or(Invalid::WrongType {
            field: "intent_summary",
            expected: "a string",
        })?
        .trim();
    if summary.is_empty() {
        return Err(Invalid::EmptySummary);
    }

    let category = field("category")?.as_str().ok_or(Invalid::WrongType {
        field: "category",
        expected: "a string",
    })?;
    if !CATEGORIES.contains(&category) {
        return Err(Invalid::UnknownCategory(category.to_string()));
    }

    let risk_score = field("risk_score")?.as_i64().ok_or(Invalid::WrongType {
        field: "risk_score",
        expected: "a whole number",
    })?;
    if !SCORE_RANGE.contains(&risk_score) {
        return Err(Invalid::ScoreOutOfRange(risk_score));
    }

    let boolean = |name: &'static str| {
        field(name)?.as_bool().ok_or(Invalid::WrongType {
            field: name,
            expected: "true or false",
        })
    };

    Ok(Verdict {
        // Cut here as well as in the grammar. Two bounds on the same value, and
        // the cheap one is the one that runs on data that already arrived.
        intent_summary: summary.chars().take(SUMMARY_LIMIT).collect(),
        category: category.to_string(),
        risk_score,
        is_destructive: boolean("is_destructive")?,
        violates_sandbox: boolean("violates_sandbox")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{"intent_summary":"Lists files in the current directory.",
        "category":"filesystem","risk_score":1,"is_destructive":false,"violates_sandbox":false}"#;

    #[test]
    fn a_well_formed_answer_parses() {
        let v = parse(GOOD).expect("valid");
        assert_eq!(v.category, "filesystem");
        assert_eq!(v.risk_score, 1);
        assert!(!v.is_destructive);
        assert_eq!(v.intent_summary, "Lists files in the current directory.");
    }

    #[test]
    fn an_object_wrapped_in_chatter_is_still_read() {
        let text = format!("Here you go:\n{GOOD}\nHope that helps!");
        assert_eq!(parse(&text).expect("valid").risk_score, 1);
    }

    /// Every way the schema can be broken, each named rather than collapsed
    /// into "invalid". The reason is stored on the failed verdict, and a stored
    /// reason nobody can act on is not worth the column.
    #[test]
    fn each_way_of_being_wrong_is_reported_as_itself() {
        let cases: [(&str, Invalid); 8] = [
            ("no braces here", Invalid::NoJson),
            (r#"{"intent_summary":}"#, Invalid::NotJson(String::new())),
            (
                r#"{"category":"vcs","risk_score":1,"is_destructive":false,"violates_sandbox":false}"#,
                Invalid::Missing("intent_summary"),
            ),
            (
                r#"{"intent_summary":"x","category":"vcs","risk_score":"high","is_destructive":false,"violates_sandbox":false}"#,
                Invalid::WrongType {
                    field: "risk_score",
                    expected: "a whole number",
                },
            ),
            (
                r#"{"intent_summary":"x","category":"vcs","risk_score":9,"is_destructive":false,"violates_sandbox":false}"#,
                Invalid::ScoreOutOfRange(9),
            ),
            (
                r#"{"intent_summary":"x","category":"vcs","risk_score":0,"is_destructive":false,"violates_sandbox":false}"#,
                Invalid::ScoreOutOfRange(0),
            ),
            (
                r#"{"intent_summary":"x","category":"exfiltration","risk_score":5,"is_destructive":true,"violates_sandbox":true}"#,
                Invalid::UnknownCategory("exfiltration".into()),
            ),
            (
                r#"{"intent_summary":"   ","category":"vcs","risk_score":1,"is_destructive":false,"violates_sandbox":false}"#,
                Invalid::EmptySummary,
            ),
        ];

        for (input, expected) in cases {
            let error = parse(input).expect_err(input);
            // `NotJson` carries serde's own message, which is not ours to pin.
            let matched = match (&error, &expected) {
                (Invalid::NotJson(_), Invalid::NotJson(_)) => true,
                (a, b) => a == b,
            };
            assert!(
                matched,
                "{input}\n  expected {expected:?}\n  got      {error:?}"
            );
        }
    }

    #[test]
    fn a_summary_longer_than_the_limit_is_cut_here_too() {
        let long = "y".repeat(SUMMARY_LIMIT * 2);
        let text = format!(
            r#"{{"intent_summary":"{long}","category":"other","risk_score":2,"is_destructive":false,"violates_sandbox":false}}"#
        );
        let v = parse(&text).expect("valid");
        assert_eq!(v.intent_summary.chars().count(), SUMMARY_LIMIT);
    }

    #[test]
    fn every_category_the_schema_names_is_accepted() {
        for category in CATEGORIES {
            let text = format!(
                r#"{{"intent_summary":"x","category":"{category}","risk_score":3,"is_destructive":false,"violates_sandbox":false}}"#
            );
            assert_eq!(parse(&text).expect(category).category, *category);
        }
    }
}
