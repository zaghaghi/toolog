//! Proof that the egress census can fail (task 7.7).
//!
//! A privacy check that cannot fail is decoration. `egress.rs` asserts that a
//! full ingest and query run opens nothing off this machine; this asserts that
//! the same census *would* have said so if it had.
//!
//! Its own binary, because the census is a question about the process: a socket
//! opened here to prove the check works would be counted by the check itself.

mod support;

use std::net::UdpSocket;

use support::{Socket, census};

#[test]
fn the_census_notices_a_socket_that_points_off_the_machine() {
    let clean = census().expect("census");
    assert!(
        clean.iter().all(Socket::is_local_only),
        "this process already held a non-loopback socket, so the rest of this \
         test would prove nothing: {clean:#?}"
    );

    // TEST-NET-3 (RFC 5737): reserved for documentation and routed nowhere.
    // `connect` on a UDP socket only records the peer — nothing is sent, so
    // this produces exactly the shape the guarantee forbids without touching a
    // network.
    let socket = UdpSocket::bind("0.0.0.0:0").expect("bind");
    socket.connect("203.0.113.1:9").expect("record the peer");

    let seen = census().expect("census");
    let escaped: Vec<&Socket> = seen.iter().filter(|s| !s.is_local_only()).collect();
    assert!(
        !escaped.is_empty(),
        "a socket deliberately pointed off the machine and the census did not \
         see it — so it is not detecting egress in the other test either:\n{seen:#?}"
    );

    drop(socket);
}
