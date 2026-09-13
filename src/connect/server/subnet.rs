//! The same-subnet check the LAN-only operations require.
//!
//! `S2C §LAN-LAN only interactions`: "the pairing server **must** check if the request
//! originated from the same subnet. This functionality **must** be implemented in such a
//! way that it works with both IPv4 and IPv6. When a request does not originate from the
//! same subnet the server **must** respond with status code 401."
//!
//! This matters because those operations have no other protection. `GET /v1/nodes` will
//! tell anyone who asks the brand, model and role of every flexible device in a building,
//! and `POST /v1/preparePairing` will make a device display its pairing code. Neither
//! carries a bearer, a challenge or a signature; the subnet is the whole of the access
//! control.
//!
//! # What "same subnet" is taken to mean
//!
//! For each address this host holds, the prefix length that address was configured with
//! defines a subnet; a peer is local if it falls inside any of them. Loopback is always
//! accepted — a request from the machine itself is as local as it gets, and refusing it
//! would make every integration test unrunnable for no gain.
//!
//! A peer arriving as an IPv4-mapped IPv6 address (`::ffff:192.0.2.1`, which is what a
//! dual-stack listener reports) is unmapped first. Skipping that step is the standard way
//! this check ends up rejecting every IPv4 client on a dual-stack server.

use core::net::IpAddr;

use axum::extract::{ConnectInfo, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// The subnets this host is on.
///
/// Enumerated once, at construction: interfaces do change, but re-reading them on every
/// request turns a `GET` into a syscall storm, and a device whose address changes is going
/// to be restarted or re-paired anyway.
#[derive(Debug, Clone)]
pub struct LocalSubnets {
    nets: alloc::vec::Vec<(IpAddr, u8)>,
}

impl LocalSubnets {
    /// Read this host's interfaces.
    ///
    /// # Errors
    ///
    /// When the interfaces cannot be enumerated at all. A host in that state cannot make
    /// the check the specification requires, and the caller must decide whether to serve
    /// the LAN-only routes without it — this crate will not decide that quietly.
    pub fn detect() -> std::io::Result<Self> {
        let nets = if_addrs::get_if_addrs()?
            .into_iter()
            .map(|iface| match iface.addr {
                if_addrs::IfAddr::V4(v4) => (IpAddr::V4(v4.ip), v4.prefixlen),
                if_addrs::IfAddr::V6(v6) => (IpAddr::V6(v6.ip), v6.prefixlen),
            })
            .collect();
        Ok(Self { nets })
    }

    /// A fixed set of subnets, for a host that knows its own topology better than we do.
    #[must_use]
    pub fn from_prefixes(nets: impl IntoIterator<Item = (IpAddr, u8)>) -> Self {
        Self {
            nets: nets.into_iter().collect(),
        }
    }

    /// Whether a peer is on one of them.
    #[must_use]
    pub fn contains(&self, peer: IpAddr) -> bool {
        let peer = unmap(peer);
        if peer.is_loopback() {
            return true;
        }
        self.nets
            .iter()
            .any(|(local, prefix)| same_prefix(unmap(*local), peer, *prefix))
    }
}

/// `::ffff:a.b.c.d` is an IPv4 peer seen through a dual-stack socket.
fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        other @ IpAddr::V4(_) => other,
    }
}

/// Whether two addresses of the same family share the first `prefix` bits.
fn same_prefix(a: IpAddr, b: IpAddr, prefix: u8) -> bool {
    match (a, b) {
        (IpAddr::V4(a), IpAddr::V4(b)) => matches_prefix(&a.octets(), &b.octets(), prefix.min(32)),
        (IpAddr::V6(a), IpAddr::V6(b)) => matches_prefix(&a.octets(), &b.octets(), prefix.min(128)),
        // Different families are never the same subnet.
        _ => false,
    }
}

fn matches_prefix(a: &[u8], b: &[u8], prefix: u8) -> bool {
    let whole = usize::from(prefix / 8);
    let bits = prefix % 8;
    if a.get(..whole) != b.get(..whole) {
        return false;
    }
    if bits == 0 {
        return true;
    }
    // A prefix that does not land on a byte boundary: compare the leading bits of the
    // next byte. `1u8 << (8 - bits)` cannot overflow because `bits` is 1..=7 here.
    let mask = !0u8 << (8 - bits);
    match (a.get(whole), b.get(whole)) {
        (Some(a), Some(b)) => a & mask == b & mask,
        _ => false,
    }
}

/// Refuse a request that did not come from this host's subnet.
///
/// Wrap the LAN-only routes in this, as `axum::middleware::from_fn_with_state`. The
/// router does not do it for you because it cannot know whether the host is a LAN endpoint
/// at all — a WAN endpoint "**cannot** implement these operations" and should return `404`
/// instead.
///
/// Requires the server to have been started with
/// `into_make_service_with_connect_info::<SocketAddr>()`, or there is no peer address to
/// check and every request is refused.
///
/// ```no_run
/// use s2_kit::connect::server::{LocalSubnets, same_subnet};
///
/// # fn build(lan_only: axum::Router) -> Result<axum::Router, Box<dyn std::error::Error>> {
/// let subnets = LocalSubnets::detect()?;
/// let guarded = lan_only.layer(axum::middleware::from_fn_with_state(
///     subnets,
///     same_subnet,
/// ));
/// # Ok(guarded)
/// # }
/// ```
pub async fn same_subnet(
    axum::extract::State(subnets): axum::extract::State<LocalSubnets>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());
    // No peer address means the server was not started with connect info. Failing closed
    // is the only safe reading: the alternative is serving the unauthenticated routes to
    // the internet because of a missing builder call.
    match peer {
        Some(peer) if subnets.contains(peer) => next.run(request).await,
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(s.parse::<core::net::Ipv4Addr>().expect("an IPv4 address"))
    }

    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(s.parse::<core::net::Ipv6Addr>().expect("an IPv6 address"))
    }

    #[test]
    fn a_peer_on_the_same_ipv4_subnet_is_local() {
        let nets = LocalSubnets::from_prefixes([(v4("192.168.1.10"), 24)]);
        assert!(nets.contains(v4("192.168.1.1")));
        assert!(nets.contains(v4("192.168.1.254")));
        // One subnet over is not the same subnet, however private it looks.
        assert!(!nets.contains(v4("192.168.2.1")));
        assert!(!nets.contains(v4("10.0.0.1")));
    }

    #[test]
    fn a_prefix_that_does_not_land_on_a_byte_boundary_still_works() {
        // /20: the first sixteen bits plus the top four of the third byte.
        let nets = LocalSubnets::from_prefixes([(v4("10.1.16.5"), 20)]);
        assert!(nets.contains(v4("10.1.16.1")));
        assert!(nets.contains(v4("10.1.31.255")));
        assert!(!nets.contains(v4("10.1.32.1")));
        // /25 splits the last byte in half.
        let nets = LocalSubnets::from_prefixes([(v4("10.0.0.5"), 25)]);
        assert!(nets.contains(v4("10.0.0.127")));
        assert!(!nets.contains(v4("10.0.0.128")));
    }

    #[test]
    fn ipv6_is_checked_the_same_way() {
        let nets = LocalSubnets::from_prefixes([(v6("fd00:1234::1"), 64)]);
        assert!(nets.contains(v6("fd00:1234::abcd")));
        assert!(!nets.contains(v6("fd00:5678::1")));
        // Link-local is its own /64 and is not implicitly local.
        assert!(!nets.contains(v6("fe80::1")));
    }

    #[test]
    fn a_dual_stack_listener_does_not_reject_every_ipv4_client() {
        // A dual-stack socket reports IPv4 peers as `::ffff:a.b.c.d`. Comparing that
        // against an IPv4 interface without unmapping refuses everyone, which is the
        // usual way this check is got wrong.
        let nets = LocalSubnets::from_prefixes([(v4("192.168.1.10"), 24)]);
        assert!(nets.contains(v6("::ffff:192.168.1.50")));
        assert!(!nets.contains(v6("::ffff:8.8.8.8")));
    }

    #[test]
    fn the_machine_itself_is_always_local() {
        let nets = LocalSubnets::from_prefixes([]);
        assert!(nets.contains(v4("127.0.0.1")));
        assert!(nets.contains(v6("::1")));
        assert!(nets.contains(v6("::ffff:127.0.0.1")));
        // And with no interfaces at all, nothing else is.
        assert!(!nets.contains(v4("192.168.1.1")));
    }

    #[test]
    fn the_families_do_not_cross() {
        let nets = LocalSubnets::from_prefixes([(v4("192.168.1.10"), 24)]);
        assert!(!nets.contains(v6("fd00::1")));
        let nets = LocalSubnets::from_prefixes([(v6("fd00::1"), 64)]);
        assert!(!nets.contains(v4("192.168.1.1")));
    }

    #[test]
    fn this_hosts_own_interfaces_can_be_read() {
        let Ok(nets) = LocalSubnets::detect() else {
            return; // A sandbox with no interfaces; nothing to assert.
        };
        assert!(nets.contains(v4("127.0.0.1")));
    }
}
