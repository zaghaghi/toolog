//! The HTTP surface: routes, encodings, error handling and the loopback bind.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use prost::Message;
use toolog_core::{Db, query};
use toolog_otlp::server::Collector;
use toolog_otlp::testing;

/// Bind an ephemeral loopback port so tests never fight over one.
fn ephemeral() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
}

struct Harness {
    handle: toolog_otlp::CollectorHandle,
    db_path: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

impl Harness {
    async fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("t.db");
        let db = Db::open(&db_path).expect("open");
        let handle = Collector::start(db, ephemeral()).await.expect("start");
        Self {
            handle,
            db_path,
            _dir: dir,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.handle.endpoint())
    }

    /// Reader connection; the collector owns the writer.
    fn reader(&self) -> Db {
        Db::open(&self.db_path).expect("reader")
    }

    /// Wait for the writer thread to catch up.
    fn wait_for_calls(&self, n: i64) -> bool {
        let reader = self.reader();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if query::stats_totals(reader.conn()).map_or(0, |t| t.tool_calls) >= n {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }
}

/// How long a request may take before the test fails rather than hangs.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// A minimal HTTP client, so the test suite adds no dependency for one POST.
///
/// It is deliberately blocking, which is why every test here runs on a
/// multi-threaded runtime: on a current-thread runtime these reads would starve
/// the server task sharing the thread.
fn post(url: &str, content_type: &str, body: &[u8]) -> (u16, String) {
    use std::io::{Read, Write};

    let rest = url.strip_prefix("http://").expect("http url");
    let (authority, path) = rest
        .split_once('/')
        .map_or((rest, String::new()), |(a, p)| (a, format!("/{p}")));

    let mut stream = std::net::TcpStream::connect(authority).expect("connect");
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .expect("read timeout");
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .expect("write timeout");
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: {content_type}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("write head");
    stream.write_all(body).expect("write body");
    stream.flush().expect("flush");

    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    parse_status(&response)
}

fn get(url: &str) -> (u16, String) {
    use std::io::{Read, Write};
    let rest = url.strip_prefix("http://").expect("http url");
    let (authority, path) = rest
        .split_once('/')
        .map_or((rest, String::new()), |(a, p)| (a, format!("/{p}")));
    let mut stream = std::net::TcpStream::connect(authority).expect("connect");
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .expect("read timeout");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )
    .expect("write");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    parse_status(&response)
}

fn parse_status(response: &str) -> (u16, String) {
    let status = response
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = response
        .split_once("\r\n\r\n")
        .map_or(String::new(), |(_, b)| b.to_string());
    (status, body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepts_protobuf_and_stores_the_call() {
    let h = Harness::start().await;
    let body = testing::request(vec![testing::tool_result("toolu_pb", true, 42)]).encode_to_vec();

    let (status, _) = post(&h.url("/v1/logs"), "application/x-protobuf", &body);
    assert_eq!(status, 200);
    assert!(h.wait_for_calls(1), "the writer thread persisted it");

    let reader = h.reader();
    let call = query::tool_call_detail(reader.conn(), "toolu_pb")
        .expect("q")
        .expect("row");
    assert_eq!(call.duration_ms, Some(42));
    assert_eq!(h.handle.counters().records, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepts_json_with_a_charset_parameter() {
    let h = Harness::start().await;
    let body = serde_json::to_vec(&testing::request(vec![testing::tool_result(
        "toolu_js", true, 7,
    )]))
    .expect("json");

    let (status, _) = post(&h.url("/v1/logs"), "application/json; charset=utf-8", &body);
    assert_eq!(status, 200);
    assert!(h.wait_for_calls(1));

    let reader = h.reader();
    assert!(
        query::tool_call_detail(reader.conn(), "toolu_js")
            .expect("q")
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_bodies_it_cannot_understand_without_dying() {
    let h = Harness::start().await;

    let (status, body) = post(&h.url("/v1/logs"), "text/plain", b"hello");
    assert_eq!(status, 415, "an unsupported type is named, not guessed at");
    assert!(body.contains("unsupported content type"));

    let (status, _) = post(&h.url("/v1/logs"), "application/json", b"{not json");
    assert_eq!(status, 400);

    let (status, _) = post(
        &h.url("/v1/logs"),
        "application/x-protobuf",
        &[0x0a, 0xff, 0x01],
    );
    assert_eq!(status, 400);

    let full = testing::request(vec![testing::tool_result("t", true, 1)]).encode_to_vec();
    let (status, _) = post(
        &h.url("/v1/logs"),
        "application/x-protobuf",
        &full[..full.len() / 2],
    );
    assert_eq!(status, 400);

    // Still serving after four bad requests.
    let good = testing::request(vec![testing::tool_result("toolu_after", true, 1)]).encode_to_vec();
    assert_eq!(
        post(&h.url("/v1/logs"), "application/x-protobuf", &good).0,
        200
    );
    assert!(h.wait_for_calls(1));
    assert_eq!(
        h.handle.counters().rejected_bodies,
        4,
        "every refusal was counted"
    );
}

/// A user who enables metrics should get a clean answer, not connection errors
/// in their Claude Code debug log.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_and_traces_are_accepted_and_dropped() {
    let h = Harness::start().await;
    for path in ["/v1/metrics", "/v1/traces"] {
        let (status, _) = post(&h.url(path), "application/x-protobuf", b"");
        assert_eq!(status, 204, "{path}");
    }
    assert_eq!(
        query::stats_totals(h.reader().conn())
            .expect("totals")
            .raw_events,
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn healthz_reports_counters() {
    let h = Harness::start().await;
    let (status, body) = get(&h.url("/healthz"));
    assert_eq!(status, 200);
    assert!(body.contains("\"status\":\"ok\""), "got {body}");
    assert!(body.contains("\"dropped\":0"));
}

/// ADR-0008 makes this a correctness property, not a default: binding any other
/// interface would let anything on the local network inject audit events.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_receiver_is_not_reachable_off_loopback() {
    let h = Harness::start().await;
    assert!(h.handle.addr().ip().is_loopback());

    // The same port on a non-loopback local address must refuse.
    let port = h.handle.addr().port();
    for candidate in local_non_loopback_addrs() {
        let target = SocketAddr::new(candidate, port);
        let result = std::net::TcpStream::connect_timeout(&target, Duration::from_millis(300));
        assert!(
            result.is_err(),
            "reachable at {target}, which ADR-0008 forbids"
        );
    }
}

/// Non-loopback addresses of this machine, if any.
fn local_non_loopback_addrs() -> Vec<IpAddr> {
    // Resolving the hostname is enough to catch a `0.0.0.0` bind without adding
    // a network-interface dependency.
    use std::net::ToSocketAddrs;
    let Ok(hostname) = std::process::Command::new("hostname").output() else {
        return Vec::new();
    };
    let host = String::from_utf8_lossy(&hostname.stdout).trim().to_string();
    if host.is_empty() {
        return Vec::new();
    }
    format!("{host}:0")
        .to_socket_addrs()
        .map(|it| it.map(|a| a.ip()).filter(|ip| !ip.is_loopback()).collect())
        .unwrap_or_default()
}

/// Exit criterion: stopping the receiver mid-session loses nothing already
/// received, and a fresh one resumes into the same database.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restarting_the_receiver_keeps_what_it_had() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("t.db");

    {
        let db = Db::open(&db_path).expect("open");
        let mut handle = Collector::start(db, ephemeral()).await.expect("start");
        let body =
            testing::request(vec![testing::tool_result("toolu_before", true, 1)]).encode_to_vec();
        assert_eq!(
            post(
                &format!("{}/v1/logs", handle.endpoint()),
                "application/x-protobuf",
                &body
            )
            .0,
            200
        );

        let reader = Db::open(&db_path).expect("reader");
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if query::stats_totals(reader.conn()).map_or(0, |t| t.tool_calls) >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        handle.shutdown();
    }

    // A new collector on a new port, same database.
    let db = Db::open(&db_path).expect("reopen");
    let handle = Collector::start(db, ephemeral()).await.expect("restart");
    let body = testing::request(vec![testing::tool_result("toolu_after", true, 2)]).encode_to_vec();
    assert_eq!(
        post(
            &format!("{}/v1/logs", handle.endpoint()),
            "application/x-protobuf",
            &body
        )
        .0,
        200
    );

    let reader = Db::open(&db_path).expect("reader");
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if query::stats_totals(reader.conn()).map_or(0, |t| t.tool_calls) >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    assert!(
        query::tool_call_detail(reader.conn(), "toolu_before")
            .expect("q")
            .is_some()
    );
    assert!(
        query::tool_call_detail(reader.conn(), "toolu_after")
            .expect("q")
            .is_some()
    );
}

/// A per-signal OTLP endpoint is used verbatim, so a value written without the
/// path posts to the root. Accepting it there costs nothing and saves a silent
/// misconfiguration — the failure mode that cost this phase an end-to-end run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logs_posted_to_the_root_path_are_accepted() {
    let h = Harness::start().await;
    let body = testing::request(vec![testing::tool_result("toolu_root", true, 3)]).encode_to_vec();

    let (status, _) = post(
        &format!("{}/", h.handle.endpoint()),
        "application/x-protobuf",
        &body,
    );
    assert_eq!(status, 200);
    assert!(h.wait_for_calls(1));

    let reader = h.reader();
    assert!(
        query::tool_call_detail(reader.conn(), "toolu_root")
            .expect("q")
            .is_some()
    );
}
