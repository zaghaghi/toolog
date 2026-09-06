//! Task 10.2's budget, measured rather than assumed.
//!
//! Phase 5 set 200 ms for the list's first paint; the histogram sits above that
//! list and loads with it, so it gets the same budget. Run against the real
//! store:
//!
//! ```text
//! cargo run --release -p toolog-core --example measure_histogram -- <db>
//! ```

use std::time::Instant;

use toolog_core::model::TimelineFilter;
use toolog_core::{Db, query};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: measure_histogram <db>");
    let db = Db::open(&path).expect("open");
    let conn = db.conn();

    let total = query::timeline_count(conn, &TimelineFilter::default()).expect("count");
    println!("{path}\n{total} calls in the store\n");

    let cases: [(&str, TimelineFilter); 3] = [
        ("no bounds (the whole store)", TimelineFilter::default()),
        (
            "one project",
            TimelineFilter {
                project_path: query::facets(conn)
                    .expect("facets")
                    .projects
                    .first()
                    .cloned(),
                ..TimelineFilter::default()
            },
        ),
        (
            "a full-text term",
            TimelineFilter {
                query: Some("cargo".to_string()),
                ..TimelineFilter::default()
            },
        ),
    ];

    // Task 12.11: the risk filter compiles a dozen LIKE/GLOB patterns into the
    // timeline's selection, and this is the first time that lands inside a
    // per-bucket GROUP BY.
    let ruleset = toolog_core::rules::load(None).expect("rules");
    let dismissed = toolog_core::rules::dismissed_rules(conn).expect("dismissed");
    let risky = TimelineFilter {
        risk: Some("high".to_string()),
        ..TimelineFilter::default()
    };
    let lens = query::Lens::with_rules(&risky, &ruleset, &dismissed);
    let _ = query::histogram(conn, lens, 0).expect("histogram");
    let mut runs: Vec<f64> = (0..5)
        .map(|_| {
            let start = Instant::now();
            let h = query::histogram(conn, lens, 0).expect("histogram");
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(h);
            ms
        })
        .collect();
    runs.sort_by(f64::total_cmp);
    let risk_h = query::histogram(conn, lens, 0).expect("histogram");
    println!(
        "{:<28} {:>7.1} ms   {:>3} columns of {:?}, {} calls",
        "@risk:high (no bounds)",
        runs[2],
        risk_h.buckets.len(),
        risk_h.size,
        risk_h.buckets.iter().map(|b| b.calls).sum::<i64>(),
    );

    for (name, filter) in cases {
        // Warm the page cache the way a second tab switch would find it, then
        // report the median of five: one cold number is a story, not a budget.
        let _ = query::histogram(conn, &filter, 0).expect("histogram");
        let mut runs: Vec<f64> = (0..5)
            .map(|_| {
                let start = Instant::now();
                let h = query::histogram(conn, &filter, 0).expect("histogram");
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                std::hint::black_box(h);
                ms
            })
            .collect();
        runs.sort_by(f64::total_cmp);

        let h = query::histogram(conn, &filter, 0).expect("histogram");
        println!(
            "{name:<28} {:>7.1} ms   {:>3} columns of {:?}",
            runs[2],
            h.buckets.len(),
            h.size,
        );
    }
}
