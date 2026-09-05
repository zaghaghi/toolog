//! Phase 7.2 — what would the redaction patterns actually do to a real store?
//!
//! A pattern set is a guess until it is run against real commands. This reads
//! an existing store **read-only**, applies the patterns to every stored
//! summary, and reports how many rows each one would change with examples of
//! each, so a pattern that fires on ordinary work is visible before it is
//! turned loose on anything.
//!
//! ```text
//! cargo run --release -p toolog-core --example measure_redaction -- ~/Library/Application\ Support/toolog/toolog.db
//! ```
//!
//! Nothing is written. Redacting an existing store means re-projecting it,
//! which is a separate, deliberate act.

use std::collections::BTreeMap;
use std::path::PathBuf;

use toolog_core::{Db, redact};

/// How many examples to print per pattern.
const EXAMPLES: usize = 4;
/// How much of a command to show.
const WIDTH: usize = 110;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: measure_redaction <path to toolog.db>");
        std::process::exit(2);
    };

    let db = Db::open(&path)?;
    let redactor = redact::load(None)?;
    println!(
        "{} patterns, {} of them broken\n",
        redactor.len(),
        redactor.broken().len()
    );

    // Commands and result bodies both. Results are the riskier surface by far
    // — `cat .env` puts its output here — and the one a measurement over
    // commands alone would say nothing about.
    for (what, column) in [
        ("commands", "input_summary"),
        ("result bodies", "result_text"),
    ] {
        report(&db, &redactor, what, column)?;
    }
    Ok(())
}

fn report(
    db: &Db,
    redactor: &redact::Redactor,
    what: &str,
    column: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // The column name is one of two literals above, never input.
    let sql = format!("SELECT {column} FROM tool_call WHERE {column} IS NOT NULL");
    let mut stmt = db.conn().prepare(&sql)?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;

    let mut total = 0usize;
    let mut changed = 0usize;
    // Which pattern fired, how often, and on what.
    let mut hits: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();

    for row in rows {
        let text = row?;
        total += 1;
        let redacted = redactor.text(&text);
        if redacted.as_ref() == text {
            continue;
        }
        changed += 1;

        for pattern in redactor.patterns() {
            let marker = format!("[redacted: {}]", pattern.id);
            if !redacted.contains(&marker) {
                continue;
            }
            let entry = hits.entry(pattern.id.clone()).or_insert((0, Vec::new()));
            entry.0 += 1;
            if entry.1.len() < EXAMPLES {
                entry.1.push(around(&text, &redacted, &marker));
            }
        }
    }

    println!("== {what}: {changed} of {total} would be changed\n");
    let mut ranked: Vec<_> = hits.into_iter().collect();
    ranked.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));

    for (id, (count, examples)) in ranked {
        println!("{id}  ({count})");
        for example in examples {
            println!("    {example}");
        }
        println!();
    }
    Ok(())
}

/// The text around a replacement, before and after.
///
/// The first line is not enough: `input_summary` holds whole heredoc bodies,
/// so a pattern can fire hundreds of characters in, and what matters is whether
/// the change reads as a redaction or as damage.
fn around(before: &str, after: &str, marker: &str) -> String {
    let at = after.find(marker).unwrap_or(0);
    let start = at.saturating_sub(WIDTH / 2);
    format!(
        "…{}…\n      → …{}…",
        window(before, start, WIDTH),
        window(after, start, WIDTH)
    )
}

/// `len` characters of `text` from byte `from`, with newlines shown.
fn window(text: &str, from: usize, len: usize) -> String {
    let from = (0..=from.min(text.len()))
        .rev()
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(0);
    text[from..]
        .chars()
        .take(len)
        .collect::<String>()
        .replace('\n', "⏎")
}
