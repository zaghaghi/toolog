//! Values that must have exactly one definition in the workspace.

/// Product name. Used for the binary, the bundle, the data directory and the
/// Homebrew cask.
///
/// Phase 0 task 0.7 settled this: `toolog` was free on crates.io and as a
/// Homebrew cask. Renaming means changing this constant and the package names,
/// and nothing else.
pub const APP_NAME: &str = "toolog";

/// Default bind address for the embedded OTLP receiver.
///
/// Loopback is not a default but a requirement ([ADR-0008]): binding any other
/// interface would let anything on the local network inject fabricated audit
/// events. A CI test asserts no non-loopback socket is ever opened.
///
/// The port is deliberately *not* the conventional 4318. [ADR-0006] writes this
/// endpoint into Claude Code's own configuration, so there is nothing to gain by
/// squatting the standard port and real cost if the user already runs a
/// collector there.
///
/// [ADR-0006]: ../../../docs/adr/0006-configure-via-settings-env-block.md
/// [ADR-0008]: ../../../docs/adr/0008-local-only-zero-egress.md
pub const DEFAULT_OTLP_HOST: &str = "127.0.0.1";

/// Default port for the embedded OTLP receiver. See [`DEFAULT_OTLP_HOST`].
pub const DEFAULT_OTLP_PORT: u16 = 47318;

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0008 makes loopback binding a correctness property, not a default.
    /// This is the cheap version of the Phase 7 egress test.
    #[test]
    fn otlp_host_is_loopback() {
        let addr: std::net::IpAddr = DEFAULT_OTLP_HOST.parse().expect("valid IP literal");
        assert!(addr.is_loopback(), "OTLP receiver must bind loopback only");
    }
}
