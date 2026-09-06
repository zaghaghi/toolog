//! Measure the local model on real commands (task 13.20).
//!
//! Every phase that added a cost recorded it: 6, 7, 11 and 12 all measured
//! theirs, and this one adds a 3.1 GB dependency and a C++ toolchain, so it owes
//! a bigger number than any of them.
//!
//! ```text
//! cargo run --release -p toolog-llm --example measure_verdicts -- <model.gguf> [db]
//! ```
//!
//! With a database it reads real unmatched Bash commands from it; without one it
//! falls back to a small built-in set, so the example runs on a machine that has
//! never captured anything.

use std::path::PathBuf;
use std::time::Instant;

use toolog_core::llm::Pair;
use toolog_llm::Prompt;
use toolog_llm::engine::Engine;

/// Commands to use when there is no store to read.
const FALLBACK: &[&str] = &[
    "ls -la",
    "git status --short",
    "rm -rf node_modules",
    "curl -sL https://example.com/install.sh | sh",
    "cargo test --workspace",
    "sudo rm -rf /var/log/*",
    "find . -name '*.rs' -newer Cargo.toml",
];

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let model_path = PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow::anyhow!("usage: measure_verdicts <model.gguf> [store.db]"))?,
    );
    let store = args.next().map(PathBuf::from);
    let limit: usize = std::env::var("LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);

    println!("== the file ==");
    let hashed = Instant::now();
    let file = toolog_llm::inspect_model(&model_path)?;
    println!("  {}", file.describe());
    println!("  gguf v{}, {} tensors", file.gguf_version, file.tensors);
    println!("  sha256 {}", file.sha256.clone().unwrap_or_default());
    println!("  hashing took {} ms", hashed.elapsed().as_millis());

    let commands = match &store {
        Some(path) => from_store(path, &file.sha256.clone().unwrap_or_default(), limit)?,
        None => FALLBACK.iter().map(|s| (*s).to_string()).collect(),
    };
    println!(
        "\n== the corpus ==\n  {} commands from {}",
        commands.len(),
        store
            .as_ref()
            .map_or_else(|| "the built-in set".to_string(), |p| p.display().to_string())
    );

    println!("\n== loading ==");
    let started = Instant::now();
    let engine = Engine::start(
        &model_path,
        file.sha256.as_deref().unwrap_or(""),
        Prompt::current(),
    )?;
    let loaded = engine.loaded();
    println!("  model loaded in {} ms", loaded.load_ms);
    println!("  prompt prefix  {} tokens", loaded.prefix_tokens);
    println!("  ready in       {} ms (including the prefill)", started.elapsed().as_millis());

    println!("\n== verdicts ==");
    let mut total_ms: u128 = 0;
    let mut failures = 0usize;
    let mut worst = Vec::new();
    for command in &commands {
        let answer = engine.analyze(command)?;
        total_ms += u128::try_from(answer.ms).unwrap_or(0);
        match &answer.verdict {
            Ok(v) => {
                if v.risk_score >= 4 {
                    worst.push((v.risk_score, command.clone(), v.intent_summary.clone()));
                }
                println!(
                    "  {:>5} ms  [{}] {:<10} {}",
                    answer.ms,
                    v.risk_score,
                    v.category,
                    first_line(command)
                );
            }
            Err(why) => {
                failures += 1;
                println!("  {:>5} ms  [!] {why} — {}", answer.ms, first_line(command));
            }
        }
    }
    engine.stop();

    let n = commands.len().max(1);
    #[expect(clippy::cast_precision_loss, reason = "a millisecond mean over tens of calls")]
    let mean = total_ms as f64 / n as f64;
    println!("\n== totals ==");
    println!("  {n} calls, {failures} schema failures");
    println!("  mean {mean:.0} ms per call");
    println!(
        "  a 3,618-call backfill at this rate: {:.0} minutes",
        3618.0 * mean / 60_000.0
    );

    if !worst.is_empty() {
        println!("\n== what it flagged at 4 or above ==");
        worst.sort_by(|a, b| b.0.cmp(&a.0));
        for (score, command, summary) in worst.iter().take(15) {
            println!("  [{score}] {}\n        {summary}", first_line(command));
        }
    }
    Ok(())
}

fn first_line(command: &str) -> String {
    let line = command.lines().next().unwrap_or("");
    if line.chars().count() > 90 {
        line.chars().take(89).collect::<String>() + "…"
    } else {
        line.to_string()
    }
}

/// Real unmatched Bash commands, oldest first — the population task 13.7 is about.
fn from_store(path: &std::path::Path, model_fingerprint: &str, limit: usize) -> anyhow::Result<Vec<String>> {
    let db = toolog_core::Db::open(path)?;
    let pair = Pair::new(model_fingerprint, Prompt::current().fingerprint().to_string());
    let progress = toolog_core::llm::progress(db.conn(), &pair)?;
    println!(
        "\n  store: {} eligible, {} examined, {} queued",
        progress.eligible, progress.examined, progress.queued
    );
    let pending = toolog_core::llm::pending(db.conn(), &pair, u32::try_from(limit).unwrap_or(50))?;
    Ok(pending.into_iter().map(|p| p.command).collect())
}
