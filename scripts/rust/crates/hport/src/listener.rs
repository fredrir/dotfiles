use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use serde::{Deserialize, Serialize};

// Forwards are ssh-owned, so skipping ssh stops the two daemons re-exporting each other's imports
const LOOP_GUARD: &str = "ssh";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Listener {
    pub port: u16,
    pub address: IpAddr,
    pub process: String,
    pub pid: u32,
}

impl Listener {
    pub fn reachable_through_loopback(&self) -> bool {
        self.address.is_loopback() || self.address.is_unspecified()
    }

    pub fn exportable(&self) -> bool {
        self.port != 0 && self.reachable_through_loopback() && self.process != LOOP_GUARD
    }

    fn target(&self) -> IpAddr {
        match self.address {
            IpAddr::V4(address) if address.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(address) if address.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            address => address,
        }
    }
}

pub fn exportable(listeners: impl IntoIterator<Item = Listener>) -> Vec<Listener> {
    listeners
        .into_iter()
        .filter(Listener::exportable)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn scan() -> Result<Vec<Listener>, String> {
    let sockets = listeners::get_all().map_err(|error| format!("listeners: {error}"))?;
    Ok(sockets
        .into_iter()
        .filter(|socket| {
            socket.protocol == listeners::Protocol::TCP
                && socket.state == listeners::SocketState::Listen
        })
        .map(|socket| Listener {
            port: socket.socket.port(),
            address: socket.socket.ip(),
            process: socket.process.name,
            pid: socket.process.pid,
        })
        .chain(crate::docker::proxies())
        .collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub port: u16,
    pub process: String,
    pub target: SocketAddr,
}

// Prefer IPv4; only a port listening on IPv6 alone is reached over ::1
pub fn services(listeners: &[Listener]) -> Vec<Service> {
    listeners
        .iter()
        .map(|listener| listener.port)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|port| {
            let on_port = || {
                listeners
                    .iter()
                    .filter(move |listener| listener.port == port)
            };
            let chosen = on_port()
                .find(|listener| listener.target().is_ipv4())
                .or_else(|| on_port().next())?;
            Some(Service {
                port,
                process: chosen.process.clone(),
                target: SocketAddr::new(chosen.target(), port),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "../tests/unit/listener_tests.rs"]
mod tests;
