//! Shared by the egress tests (task 7.7).
//!
//! The census is a question about the *process*, so the guarantee and the
//! proof-that-it-can-fail have to run in separate test binaries. This is the
//! part they both need.

#![allow(
    dead_code,
    unreachable_pub,
    reason = "a shared test module: each binary uses part of it, and `pub` is \
              how a `mod` in a test binary exposes anything at all"
)]

use std::net::IpAddr;
use std::path::{Path, PathBuf};

/// One socket this process holds, as the operating system describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Socket {
    /// For the report, so a failure names what it found.
    pub description: String,
    /// Every address on it — local and remote.
    pub addresses: Vec<IpAddr>,
}

impl Socket {
    #[must_use]
    pub fn is_local_only(&self) -> bool {
        self.addresses.iter().all(|ip| {
            // An unspecified remote (0.0.0.0 / ::) is how both platforms
            // describe "no peer", not a route off the machine.
            ip.is_loopback() || ip.is_unspecified()
        })
    }
}

/// The internet sockets belonging to this process.
///
/// `None` when the platform has no way to ask, which fails the test rather than
/// passing it quietly — a privacy guarantee that silently stops being checked
/// is worse than one that was never claimed.
pub fn census() -> Option<Vec<Socket>> {
    #[cfg(target_os = "linux")]
    return linux::sockets();
    #[cfg(target_os = "macos")]
    return macos::sockets();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return None;
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Socket;
    use std::net::IpAddr;

    /// `lsof` lists this process's internet sockets with both endpoints.
    pub(super) fn sockets() -> Option<Vec<Socket>> {
        let pid = std::process::id();
        let out = std::process::Command::new("lsof")
            .args(["-nP", "-a", "-i", "-p", &pid.to_string()])
            .output()
            .ok()?;
        // `lsof` exits non-zero when it finds nothing, which is the good case.
        let text = String::from_utf8_lossy(&out.stdout).into_owned();

        Some(
            text.lines()
                .skip(1)
                .filter_map(|line| {
                    let name = line.split_whitespace().nth(8)?;
                    Some(Socket {
                        description: line.trim().to_string(),
                        addresses: name.split("->").filter_map(address).collect(),
                    })
                })
                .collect(),
        )
    }

    /// `127.0.0.1:47318` or `[::1]:47318` → the address part.
    fn address(endpoint: &str) -> Option<IpAddr> {
        let endpoint = endpoint.trim();
        if let Some(rest) = endpoint.strip_prefix('[') {
            return rest.split(']').next()?.parse().ok();
        }
        endpoint.rsplit_once(':')?.0.parse().ok()
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::Socket;
    use std::collections::HashSet;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    /// Our own socket inodes, matched against the kernel's socket tables.
    ///
    /// Matching by inode rather than reading the tables wholesale is what keeps
    /// this about *this* process on a machine running other things.
    pub(super) fn sockets() -> Option<Vec<Socket>> {
        let mine: HashSet<String> = std::fs::read_dir("/proc/self/fd")
            .ok()?
            .filter_map(|entry| std::fs::read_link(entry.ok()?.path()).ok())
            .filter_map(|target| {
                target
                    .to_str()?
                    .strip_prefix("socket:[")?
                    .strip_suffix(']')
                    .map(ToString::to_string)
            })
            .collect();

        let mut out = Vec::new();
        for table in [
            "/proc/self/net/tcp",
            "/proc/self/net/tcp6",
            "/proc/self/net/udp",
            "/proc/self/net/udp6",
        ] {
            let Ok(text) = std::fs::read_to_string(table) else {
                continue;
            };
            for line in text.lines().skip(1) {
                let fields: Vec<&str> = line.split_whitespace().collect();
                // local, remote, …, inode
                let (Some(local), Some(remote), Some(inode)) =
                    (fields.get(1), fields.get(2), fields.get(9))
                else {
                    continue;
                };
                if !mine.contains(*inode) {
                    continue;
                }
                out.push(Socket {
                    description: format!("{table} {local} -> {remote}"),
                    addresses: [local, remote].iter().filter_map(|e| address(e)).collect(),
                });
            }
        }
        Some(out)
    }

    /// `0100007F:B8B6` — the address is little-endian hex.
    fn address(endpoint: &str) -> Option<IpAddr> {
        let hex = endpoint.split(':').next()?;
        match hex.len() {
            8 => {
                let raw = u32::from_str_radix(hex, 16).ok()?;
                Some(IpAddr::V4(Ipv4Addr::from(raw.to_be())))
            }
            32 => {
                let mut octets = [0u8; 16];
                for (i, slot) in octets.iter_mut().enumerate() {
                    *slot = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
                }
                // Each 32-bit word is little-endian on the wire.
                for word in octets.chunks_exact_mut(4) {
                    word.reverse();
                }
                Some(IpAddr::V6(Ipv6Addr::from(octets)))
            }
            _ => None,
        }
    }
}

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}
