//! The zero-egress guarantee, as a build failure rather than a convention
//! (task 7.7, [ADR-0008]).
//!
//! [PRIVACY.md](../../../PRIVACY.md) says nothing leaves the machine. This runs
//! the workload that would betray that if anything did — a full ingest through
//! both lanes, then every query the window issues — and then asks the operating
//! system what sockets this process actually has. Any socket with a
//! non-loopback address on either end fails the test.
//!
//! Three checks, because each catches what the others cannot:
//!
//! 1. **The socket census** is behavioural: it sees what the process really
//!    opened, including anything a dependency opened without us asking. Its
//!    limit is that a connection opened and closed entirely between the
//!    workload and the census would not be caught.
//! 2. **The manifest check** closes part of that: nothing in this workspace
//!    asks for an HTTP client, so the obvious way to add egress fails here
//!    before it can be written.
//! 3. **The source check** closes the remaining hole: outbound connections
//!    hand-rolled on `std::net`, which need no dependency at all.
//!
//! That the census can fail at all is asserted separately, in
//! `egress_control.rs`. It has to be a separate test binary: the census is a
//! question about the *process*, and a socket opened to prove the check works
//! would be visible to the check itself.
//!
//! [ADR-0008]: ../../../docs/adr/0008-local-only-zero-egress.md

mod support;

use std::path::{Path, PathBuf};

use support::{Socket, census, workspace_root};
use toolog_core::model::{Page, TimelineFilter};
use toolog_core::{Db, chain, query, rules, verify};
use toolog_ingest::Backfill;

/// Ingest both lanes and run every query the window issues.
fn full_workload() {
    let db = Db::open_in_memory().expect("open");
    let conn = db.conn();

    // The transcript lane, from the checked-in fixtures.
    let transcripts = workspace_root().join("fixtures").join("transcripts");
    Backfill::new(conn).run(&transcripts).expect("backfill");

    // The OTLP lane, without a server: the records are what matter here, and
    // the receiver's own binding is asserted in toolog-otlp's tests.
    let records = vec![
        toolog_otlp::testing::tool_decision("toolu_egress", "reject", "config"),
        toolog_otlp::testing::tool_decision("toolu_egress2", "accept", "user_temporary"),
    ];
    toolog_otlp::ingest_records(conn, "otlp:egress", &records).expect("otlp");

    // Every read the three views make. Phase 9 took two views away; the list
    // that replaced theirs is the current one, not the old one minus three —
    // the guarantee is only as good as what actually runs here.
    let filter = TimelineFilter::default();
    query::timeline_rows(conn, &filter, Page::default()).expect("timeline");
    query::timeline_count(conn, &filter).expect("count");
    query::timeline_groups(conn, &filter).expect("groups");
    query::facets(conn).expect("facets");
    query::list_sessions(conn, Page::default()).expect("sessions");
    query::stats_totals(conn).expect("totals");
    query::stats_tool_usage(conn).expect("tools");
    query::reconcile(conn).expect("reconcile");
    query::ingest_summary(conn).expect("ingest summary");
    query::search(conn, "rm", Page::default()).expect("search");
    // The timeline's activity histogram (Phase 10), which loads with the list.
    query::histogram(conn, &filter, 0).expect("histogram");

    // And the same reads narrowed by risk (Phase 12), which compile the rules
    // into the timeline's own selection.
    let risky = TimelineFilter {
        risk: Some("high".to_string()),
        ..TimelineFilter::default()
    };
    let ruleset = rules::load(None).expect("rules");
    let dismissed = rules::dismissed_rules(conn).expect("dismissed");
    let lens = query::Lens::with_rules(&risky, &ruleset, &dismissed);
    query::timeline_rows(conn, lens, Page::default()).expect("risk timeline");
    query::timeline_count(conn, lens).expect("risk count");
    query::histogram(conn, lens, 0).expect("risk histogram");

    // The detail pane, which reads a call, its session, its diffs and the
    // transcript line behind it.
    if let Some(call) = query::tool_call_detail(conn, "toolu_egress").expect("detail") {
        if let Some(id) = call.session_id.as_deref() {
            query::session(conn, id).expect("session");
        }
        query::file_changes(conn, &call.tool_use_id).expect("file changes");
        query::source_record(conn, &call).expect("source record");
    }

    let findings = rules::evaluate(conn, &ruleset).expect("evaluate");
    rules::reconcile(conn, &ruleset, &findings).expect("reconcile");
    // The sighting ledger (Phase 12), which a review writes as it goes.
    let mut seen = findings.clone();
    rules::record_sightings(conn, &ruleset, &mut seen, 0).expect("sightings");
    // The risk view's drill-through past a finding's examples.
    if let Some(rule) = ruleset.first() {
        rules::calls(conn, rule, Page::default()).expect("rule calls");
    }

    verify::completeness(conn).expect("completeness");
    chain::verify(conn).expect("chain");
}

/// The check ADR-0008 promises: a full run opens nothing off this machine.
#[test]
fn a_full_ingest_and_query_run_opens_no_non_loopback_socket() {
    let before = census().expect(
        "no way to enumerate this process's sockets on this platform — the egress \
         guarantee cannot be checked, which fails rather than passes silently",
    );
    assert!(
        before.iter().all(Socket::is_local_only),
        "the test process already held a non-loopback socket before doing anything: {:#?}",
        before
            .iter()
            .filter(|s| !s.is_local_only())
            .collect::<Vec<_>>()
    );

    full_workload();

    let after = census().expect("census");
    let escaped: Vec<&Socket> = after.iter().filter(|s| !s.is_local_only()).collect();
    assert!(
        escaped.is_empty(),
        "ADR-0008 says nothing leaves this machine, and a full ingest plus query run \
         opened {} socket(s) that do:\n{escaped:#?}",
        escaped.len()
    );
}

/// Crates that exist to make outbound requests.
///
/// Not an exhaustive list of every HTTP client ever published — it is the set
/// anyone would actually reach for, and adding one to this workspace is exactly
/// the change that should have to argue with a failing test first.
///
/// `tauri-plugin-updater` is here because of what Phase 8 decided. ADR-0008
/// had reserved an update check as the one permitted exception; the phase
/// declined to take it, so there is now **no** exception, and this list says
/// so in the only place that can enforce it. The plugin would compile
/// `reqwest` and a TLS stack into every binary whether the switch were on or
/// off, which turns a structural guarantee into a runtime one — see the
/// addendum to ADR-0008.
const OUTBOUND_CLIENTS: &[&str] = &[
    "reqwest",
    "ureq",
    "isahc",
    "attohttpc",
    "curl",
    "surf",
    "hyper-tls",
    "hyper-rustls",
    "native-tls",
    "rustls",
    "openssl",
    "tauri-plugin-updater",
    "self_update",
];

/// No crate in this workspace asks for the ability to make outbound requests.
///
/// Reads the manifests, not `Cargo.lock`, and the difference is the point.
/// `Cargo.lock` is the resolved set across **every** target and optional
/// feature: `reqwest` is in ours because `tauri` declares it optionally, behind
/// a feature this workspace does not enable. `cargo tree -e normal` for the
/// host shows no `reqwest`, no `hyper-tls`, no `rustls` — it is listed, not
/// linked. Failing on the lockfile would therefore fail on day one for a crate
/// that is never compiled, and allow-listing it would gut the check.
///
/// So this asserts the thing we control and can state exactly: **nothing here
/// asks for an HTTP client**. Whether something transitively reaches for one
/// anyway is what the socket census answers, by watching rather than reading.
#[test]
fn no_manifest_in_the_workspace_asks_for_an_outbound_client() {
    let root = workspace_root();
    let mut manifests = vec![root.join("Cargo.toml")];
    if let Ok(entries) = std::fs::read_dir(root.join("crates")) {
        manifests.extend(
            entries
                .flatten()
                .map(|e| e.path().join("Cargo.toml"))
                .filter(|p| p.is_file()),
        );
    }
    assert!(
        manifests.len() > 1,
        "no crate manifests found — this check is not checking anything"
    );

    let mut offenders = Vec::new();
    for manifest in &manifests {
        let Ok(text) = std::fs::read_to_string(manifest) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            let code = line.trim();
            if code.starts_with('#') {
                continue;
            }
            // A dependency line names its crate first: `reqwest = "0.13"` or
            // `reqwest.workspace = true`.
            let Some(name) = code.split(['=', '.', ' ']).next() else {
                continue;
            };
            if OUTBOUND_CLIENTS.contains(&name) {
                offenders.push(format!("{}:{} {code}", manifest.display(), n + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "ADR-0008 rules out egress, and these manifests ask for a client that performs it:\n{}\n\
         There is no exception left to appeal to. The update check ADR-0008 had \
         reserved was evaluated in Phase 8 and declined, so v1.0 ships with no \
         network call of any kind; `brew upgrade` is the update path. Reversing \
         that means amending the ADR and the README's front page, not this list.",
        offenders.join("\n")
    );
}

/// No outbound connection hand-rolled on `std::net`, which needs no dependency.
#[test]
fn no_workspace_source_opens_an_outbound_connection() {
    // Every way std offers to start a connection. Binding and accepting are
    // fine — that is the OTLP receiver, on loopback.
    const CONNECTORS: &[&str] = &[
        "TcpStream::connect",
        "UdpSocket::connect",
        "connect_timeout",
        "UnixStream::connect",
    ];

    // The one place that connects, and it refuses anything but loopback —
    // asserted by its own test, `a_non_loopback_address_is_refused_without_
    // connecting`, rather than trusted because it is named here.
    const LOOPBACK_ONLY: &str = "health.rs";

    let mut offenders = Vec::new();
    for entry in walk(&workspace_root().join("crates")) {
        // Tests may connect — the OTLP server's own tests do, to loopback.
        let is_test = entry.components().any(|c| c.as_os_str() == "tests");
        let is_probe = entry.file_name().is_some_and(|f| f == LOOPBACK_ONLY);
        if is_test || is_probe {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&entry) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            for needle in CONNECTORS {
                if code.contains(needle) {
                    offenders.push(format!("{}:{} {code}", entry.display(), n + 1));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "ADR-0008 rules out egress, and these lines open a connection:\n{}",
        offenders.join("\n")
    );
}

/// Every `.rs` file under a directory.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}
