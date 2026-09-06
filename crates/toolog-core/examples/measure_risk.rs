//! Task 11.6: the risk review's cost, before and after.
//!
//! ```text
//! cargo run --release -p toolog-core --example measure_risk -- <db>
//! ```

use std::time::Instant;

use toolog_core::{Db, rules};

fn main() {
    let path = std::env::args().nth(1).expect("usage: measure_risk <db>");
    let db = Db::open(&path).expect("open");
    let conn = db.conn();
    let ruleset = rules::load(None).expect("rules");

    println!("{path}\n{} rules\n", ruleset.len());

    // Warm the page cache, then take the median of five: a tab activation on a
    // running application finds the store warm, not cold.
    let _ = rules::evaluate(conn, &ruleset).expect("evaluate");

    let mut whole: Vec<f64> = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let findings = rules::evaluate(conn, &ruleset).expect("evaluate");
        let projects = rules::reconcile(conn, &ruleset, &findings).expect("reconcile");
        whole.push(start.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box((findings, projects));
    }
    whole.sort_by(f64::total_cmp);

    let findings = rules::evaluate(conn, &ruleset).expect("evaluate");
    println!(
        "evaluate + reconcile    {:>8.1} ms   {} rules listed, {} of them matching",
        whole[2],
        findings.len(),
        findings.iter().filter(|f| f.calls > 0).count(),
    );

    // The memo's guard (ADR-0011), on the same store the review runs against:
    // this is what a re-opened tab costs when nothing has changed.
    let mut guard: Vec<f64> = (0..20)
        .map(|_| {
            let start = Instant::now();
            let v: i64 = conn
                .query_row("PRAGMA data_version", [], |r| r.get(0))
                .expect("pragma");
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(v);
            ms
        })
        .collect();
    guard.sort_by(f64::total_cmp);
    println!(
        "the memo's guard      {:>11.4} ms   one PRAGMA data_version",
        guard[10]
    );

    // Per rule, so a slow one is visible rather than averaged away.
    println!("\nper rule:");
    for rule in &ruleset {
        let one = std::slice::from_ref(rule);
        let _ = rules::evaluate(conn, one).expect("evaluate");
        let mut runs: Vec<f64> = (0..5)
            .map(|_| {
                let start = Instant::now();
                let f = rules::evaluate(conn, one).expect("evaluate");
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                std::hint::black_box(f);
                ms
            })
            .collect();
        runs.sort_by(f64::total_cmp);
        println!("  {:<38} {:>8.1} ms", rule.id, runs[2]);
    }
}
