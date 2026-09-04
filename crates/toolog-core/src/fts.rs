//! Turning what a person typed into an FTS5 query that will not explode.
//!
//! This exists because the corpus is mostly shell commands. `rm -rf`, `a | b`,
//! `*.rs`, `--flag`, `foo:bar` and `NOT` are all ordinary things to search for
//! and all meaningful syntax to FTS5 — a bare `MATCH` on user input either
//! errors or silently means something else.
//!
//! Every token is quoted, so the query engine treats it as a literal string.

/// Build a safe FTS5 `MATCH` expression from free-form user input.
///
/// Terms are `AND`-ed. The final term gets a prefix wildcard so search feels
/// responsive as the user types. Returns `None` when the input has no
/// searchable content, which callers should treat as "no filter" rather than
/// "no results".
pub fn build_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(quote_term)
        .collect();

    if terms.is_empty() {
        return None;
    }

    let last = terms.len() - 1;
    let joined = terms
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if i == last {
                format!("{t}*")
            } else {
                t.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" AND ");

    Some(joined)
}

/// Wrap one term in FTS5 double quotes, escaping any it contains.
///
/// Inside a quoted string FTS5 treats everything as literal, so `|`, `*`, `-`,
/// `:`, `(`, `)` and the bare keywords `AND` / `OR` / `NOT` / `NEAR` all lose
/// their meaning — which is what we want.
fn quote_term(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_words_are_quoted_and_anded() {
        assert_eq!(
            build_query("cargo test").as_deref(),
            Some(r#""cargo" AND "test"*"#)
        );
    }

    #[test]
    fn blank_input_yields_no_query() {
        assert_eq!(build_query(""), None);
        assert_eq!(build_query("   \t "), None);
    }

    #[test]
    fn embedded_quotes_are_doubled() {
        assert_eq!(build_query(r#"say"hi"#).as_deref(), Some(r#""say""hi"*"#));
    }

    /// The whole reason this module exists. Every one of these is a real thing
    /// to search a shell-command corpus for, and every one is FTS5 syntax.
    #[test]
    fn fts5_operators_survive_as_literals() {
        for hostile in [
            "rm -rf", "a | b", "*.rs", "--force", "foo:bar", "NOT", "AND", "NEAR", "(x)",
            "^anchor", "a\"b", "-", "*",
        ] {
            let q = build_query(hostile).expect("query built");
            assert!(q.starts_with('"'), "term must be quoted: {hostile} -> {q}");
        }
    }
}
