use std::net::{Ipv4Addr, SocketAddrV4};

use hostkit::{CLIP_PORT, Host, Route};

const LAN_SERVE: u16 = 8457;
const LAN_DIAL: u16 = 8458;

pub fn listen(this: Host, route: Route) -> SocketAddrV4 {
    match (this, route) {
        (_, Route::Lan) => loopback(LAN_SERVE),
        (Host::Macie, Route::Cable) => loopback(CLIP_PORT),
        (Host::Macie, Route::Wifi) => loopback(8454),
        (Host::Macie, Route::Tailscale) => loopback(8456),
        (Host::Archie, route) => SocketAddrV4::new(fixed(Host::Archie, route), CLIP_PORT),
    }
}

pub fn dial(peer: Host, route: Route) -> SocketAddrV4 {
    match route {
        Route::Lan => loopback(LAN_DIAL),
        route => SocketAddrV4::new(fixed(peer, route), CLIP_PORT),
    }
}

fn fixed(host: Host, route: Route) -> Ipv4Addr {
    host.address(route).unwrap_or(Ipv4Addr::LOCALHOST)
}

fn loopback(port: u16) -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)
}

#[cfg(test)]
#[path = "../tests/unit/endpoint_tests.rs"]
mod tests;
