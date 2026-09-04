//! Choosing a port to listen on.
//!
//! The default is deliberately **not** the conventional 4318. [ADR-0006] writes
//! this endpoint into Claude Code's own configuration, so there is nothing to
//! gain by squatting the standard port and real cost if the user already runs a
//! collector there.
//!
//! Whatever port is chosen must be written into Claude Code's `env` block too.
//! If the two disagree, capture silently stops — the exporter posts into nothing
//! and only says so in a debug log.
//!
//! [ADR-0006]: ../../../docs/adr/0006-configure-via-settings-env-block.md

use std::net::{IpAddr, SocketAddr, TcpListener};

use toolog_core::constants::{DEFAULT_OTLP_HOST, DEFAULT_OTLP_PORT};

/// How many ports past the default to try before giving up.
const FALLBACK_ATTEMPTS: u16 = 16;

/// Why no port could be chosen.
#[derive(Debug, thiserror::Error)]
pub enum PortError {
    #[error("{0} is not a valid address")]
    BadHost(String),
    #[error(
        "no free port between {first} and {last} on {host}; \
         set one explicitly if something else is using this range"
    )]
    NoneFree { host: String, first: u16, last: u16 },
}

/// The loopback address the receiver binds by default.
///
/// # Panics
///
/// Never in practice: [`DEFAULT_OTLP_HOST`] is a compile-time IP literal.
#[must_use]
pub fn default_addr() -> SocketAddr {
    SocketAddr::new(
        DEFAULT_OTLP_HOST
            .parse::<IpAddr>()
            .expect("DEFAULT_OTLP_HOST is a valid IP literal"),
        DEFAULT_OTLP_PORT,
    )
}

/// Whether a port can be bound right now.
#[must_use]
pub fn is_free(host: IpAddr, port: u16) -> bool {
    TcpListener::bind(SocketAddr::new(host, port)).is_ok()
}

/// Find a bindable port, starting at `preferred`.
///
/// Falls back to the next few ports so a stale process or an unrelated
/// collector does not stop capture entirely. The caller **must** propagate the
/// result to Claude Code's configuration.
pub fn choose(host: &str, preferred: u16) -> Result<SocketAddr, PortError> {
    let ip: IpAddr = host
        .parse()
        .map_err(|_| PortError::BadHost(host.to_string()))?;

    for offset in 0..FALLBACK_ATTEMPTS {
        let port = preferred.saturating_add(offset);
        if is_free(ip, port) {
            if offset > 0 {
                tracing::warn!(
                    preferred,
                    chosen = port,
                    "preferred port was taken; Claude Code's endpoint must be updated to match"
                );
            }
            return Ok(SocketAddr::new(ip, port));
        }
    }

    Err(PortError::NoneFree {
        host: host.to_string(),
        first: preferred,
        last: preferred.saturating_add(FALLBACK_ATTEMPTS - 1),
    })
}

/// Find a bindable port at the default host and port.
pub fn choose_default() -> Result<SocketAddr, PortError> {
    choose(DEFAULT_OTLP_HOST, DEFAULT_OTLP_PORT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0008 makes this a correctness property: binding any other interface
    /// would let anything on the local network inject fabricated audit events.
    #[test]
    fn the_default_address_is_loopback() {
        assert!(default_addr().ip().is_loopback());
        assert_eq!(default_addr().port(), DEFAULT_OTLP_PORT);
    }

    #[test]
    fn a_taken_port_falls_back_to_the_next_free_one() {
        let ip: IpAddr = DEFAULT_OTLP_HOST.parse().expect("ip");
        // Hold a port, then ask for it.
        let held = TcpListener::bind(SocketAddr::new(ip, 0)).expect("bind");
        let taken = held.local_addr().expect("addr").port();

        let chosen = choose(DEFAULT_OTLP_HOST, taken).expect("a fallback exists");
        assert_ne!(chosen.port(), taken, "the held port was skipped");
        assert!(chosen.port() > taken);
        assert!(chosen.ip().is_loopback());
    }

    #[test]
    fn a_free_port_is_used_as_is() {
        let ip: IpAddr = DEFAULT_OTLP_HOST.parse().expect("ip");

        // Releasing a port and asking for it back is a race with every other
        // test binary the workspace runs in parallel. Retry rather than assert
        // that nothing else claimed the number in between.
        for attempt in 0..5 {
            let probe = TcpListener::bind(SocketAddr::new(ip, 0)).expect("bind");
            let free = probe.local_addr().expect("addr").port();
            drop(probe);

            if choose(DEFAULT_OTLP_HOST, free).expect("a port").port() == free {
                return;
            }
            assert!(attempt < 4, "a free port was never used as-is");
        }
    }

    #[test]
    fn a_bad_host_is_reported_rather_than_guessed() {
        assert!(matches!(
            choose("not-an-ip", 1234),
            Err(PortError::BadHost(_))
        ));
    }
}
