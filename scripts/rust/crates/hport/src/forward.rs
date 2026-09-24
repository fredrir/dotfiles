use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};

pub const ALIAS: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 2);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Bind {
    Alias,
    Loopback4,
    Loopback6,
}

impl Bind {
    pub const MIRROR: [Bind; 2] = [Bind::Loopback4, Bind::Loopback6];

    pub fn address(self) -> IpAddr {
        match self {
            Bind::Alias => IpAddr::V4(ALIAS),
            Bind::Loopback4 => IpAddr::V4(Ipv4Addr::LOCALHOST),
            Bind::Loopback6 => IpAddr::V6(Ipv6Addr::LOCALHOST),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Forward {
    pub bind: Bind,
    pub port: u16,
    pub target: SocketAddr,
}

impl Forward {
    pub fn spec(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            literal(self.bind.address()),
            self.port,
            literal(self.target.ip()),
            self.target.port()
        )
    }
}

fn literal(address: IpAddr) -> String {
    match address {
        IpAddr::V4(address) => address.to_string(),
        IpAddr::V6(address) => format!("[{address}]"),
    }
}

// SO_REUSEADDR skips TIME_WAIT, but on macOS allows binds under a wildcard, so probe those too
pub fn mirror_free(port: u16) -> bool {
    probes()
        .into_iter()
        .all(|address| bindable(SocketAddr::new(address, port)))
}

fn probes() -> Vec<IpAddr> {
    let mut probes = Bind::MIRROR.map(Bind::address).to_vec();
    if cfg!(target_os = "macos") {
        probes.extend([
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        ]);
    }
    probes
}

pub fn alias_ready() -> bool {
    bindable(SocketAddr::new(IpAddr::V4(ALIAS), 0))
}

fn bindable(address: SocketAddr) -> bool {
    let Ok(socket) = Socket::new(
        Domain::for_address(address),
        Type::STREAM,
        Some(Protocol::TCP),
    ) else {
        return false;
    };
    let prepared = socket.set_reuse_address(true).is_ok()
        && (address.is_ipv4() || socket.set_only_v6(true).is_ok());
    prepared && socket.bind(&SockAddr::from(address)).is_ok()
}

#[cfg(test)]
#[path = "../tests/unit/forward_tests.rs"]
mod tests;
