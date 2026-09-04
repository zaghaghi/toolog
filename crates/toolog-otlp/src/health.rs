//! Asking a running receiver whether it is alive.
//!
//! `doctor` and the tray both need the answer, and neither should guess it from
//! whether a port is bound — something else could hold it. This performs the
//! real `GET /healthz` and reads the counters back, so "up" means our receiver
//! specifically, and the tray can show how much it has taken in.
//!
//! Hand-rolled over `TcpStream` rather than pulling in an HTTP client for one
//! loopback GET.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::server::CounterSnapshot;

/// How long to wait on a loopback health check before calling it down.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(750);

/// What a health probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// Our receiver answered, with these counters.
    Up(CounterSnapshot),
    /// Nothing accepted a connection.
    Down,
    /// Something is listening but did not answer as our receiver does.
    Foreign(String),
}

impl Health {
    #[must_use]
    pub fn is_up(&self) -> bool {
        matches!(self, Self::Up(_))
    }
}

/// Probe `GET /healthz` at `addr`.
#[must_use]
pub fn probe(addr: SocketAddr) -> Health {
    probe_with_timeout(addr, DEFAULT_TIMEOUT)
}

/// [`probe`] with an explicit timeout.
#[must_use]
pub fn probe_with_timeout(addr: SocketAddr, timeout: Duration) -> Health {
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, timeout) else {
        return Health::Down;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let request = format!(
        "GET /healthz HTTP/1.1\r\nHost: {addr}\r\nAccept: application/json\r\n\
         Connection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return Health::Foreign("no response to a health check".to_string());
    }

    let mut response = String::new();
    if stream.read_to_string(&mut response).is_err() && response.is_empty() {
        return Health::Foreign("no response to a health check".to_string());
    }

    let Some((head, body)) = response.split_once("\r\n\r\n") else {
        return Health::Foreign("unrecognized response".to_string());
    };
    if !head.starts_with("HTTP/1.1 200") {
        let status = head.lines().next().unwrap_or_default().to_string();
        return Health::Foreign(status);
    }

    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(json) if json.get("status").and_then(serde_json::Value::as_str) == Some("ok") => {
            let counters = json
                .get("counters")
                .and_then(|c| serde_json::from_value(c.clone()).ok())
                .unwrap_or_default();
            Health::Up(counters)
        }
        _ => Health::Foreign("something else is listening on this port".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, TcpListener};

    fn unused_port() -> SocketAddr {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let addr = l.local_addr().expect("addr");
        drop(l);
        addr
    }

    #[test]
    fn nothing_listening_is_down_not_an_error() {
        assert_eq!(
            probe_with_timeout(unused_port(), Duration::from_millis(200)),
            Health::Down
        );
    }

    /// A bound port is not proof our receiver is there — the whole reason this
    /// probes rather than checking `is_free`.
    #[test]
    fn another_server_on_the_port_is_reported_as_foreign() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            }
        });

        let health = probe_with_timeout(addr, Duration::from_secs(2));
        assert!(
            matches!(health, Health::Foreign(_)),
            "expected a foreign listener, got {health:?}"
        );
        assert!(!health.is_up());
    }

    #[test]
    fn a_healthy_receiver_reports_its_counters() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let body = r#"{"status":"ok","counters":{"batches":2,"records":11,"dropped":0,"rejected_bodies":0}}"#;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });

        match probe_with_timeout(addr, Duration::from_secs(2)) {
            Health::Up(counters) => assert_eq!(counters.records, 11),
            other => panic!("expected Up, got {other:?}"),
        }
    }

    #[test]
    fn the_probe_address_is_loopback_only() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 47318);
        assert!(addr.ip().is_loopback());
    }
}
