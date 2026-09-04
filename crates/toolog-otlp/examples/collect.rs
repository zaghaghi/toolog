//! Run the OTLP receiver against a real Claude Code session.
//!
//! ```text
//! cargo run -p toolog-otlp --example collect -- [seconds] [db-path]
//! ```
//!
//! Prints the environment Claude Code needs, then reports rows as they land.
//! Until Phase 4's `doctor` writes those variables into `~/.claude/settings.json`,
//! exporting them for one command is how the lane is exercised end to end.

use std::time::{Duration, Instant};

use toolog_core::model::{Page, TimelineFilter};
use toolog_core::{Db, query};
use toolog_otlp::port;
use toolog_otlp::server::Collector;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber_init();

    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let db_path = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "toolog-collect.db".into());

    let addr = port::choose_default()?;
    let handle = Collector::start(Db::open(&db_path)?, addr).await?;
    let endpoint = handle.endpoint();

    println!("listening on {endpoint}  (db: {db_path})\n");
    println!("Run Claude Code with (the /v1/logs path is required:");
    println!("a per-signal OTLP endpoint is used verbatim, never appended to):\n");
    println!("  export CLAUDE_CODE_ENABLE_TELEMETRY=1");
    println!("  export OTEL_LOGS_EXPORTER=otlp");
    println!("  export OTEL_EXPORTER_OTLP_LOGS_PROTOCOL=http/protobuf");
    println!("  export OTEL_EXPORTER_OTLP_LOGS_ENDPOINT={endpoint}/v1/logs");
    println!("  export OTEL_LOGS_EXPORT_INTERVAL=1000");
    println!("  export OTEL_LOG_TOOL_DETAILS=1\n");

    let reader = Db::open(&db_path)?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut seen = 0usize;

    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;

        let rows = query::timeline_page(
            reader.conn(),
            &TimelineFilter::default(),
            Page {
                limit: 200,
                offset: 0,
            },
        )?;
        for call in rows.iter().rev().skip(seen) {
            println!(
                "{:<10} {:<22} decision={:<8} source={:<16} {:>7} {}",
                call.tool_use_id.chars().take(10).collect::<String>(),
                call.tool_name.as_deref().unwrap_or("-"),
                call.decision.as_deref().unwrap_or("-"),
                call.decision_source.as_deref().unwrap_or("-"),
                call.duration_ms.map_or("-".into(), |d| format!("{d}ms")),
                call.input_summary.as_deref().unwrap_or("")
            );
        }
        seen = rows.len();
    }

    let totals = query::stats_totals(reader.conn())?;
    let recon = query::reconcile(reader.conn())?;
    let counters = handle.counters();

    println!("\n--- received ---");
    println!(
        "batches {} | records {} | dropped {}",
        counters.batches, counters.records, counters.dropped
    );
    #[allow(clippy::cast_precision_loss)]
    let cost_usd = totals.cost_usd_micros as f64 / 1_000_000.0;
    println!(
        "tool calls {} | api requests {} | cost ${cost_usd:.4}",
        totals.tool_calls, totals.api_requests
    );
    println!(
        "reconcile: both {} | transcript-only {} | otel-only (rejections) {}",
        recon.both, recon.transcript_only, recon.otel_only
    );
    Ok(())
}

fn tracing_subscriber_init() {
    // Deliberately quiet: the table above is the output that matters.
}
