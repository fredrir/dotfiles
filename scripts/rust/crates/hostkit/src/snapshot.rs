use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::host::{self, Host, PROBE, Route};
use crate::socket;

/// The result of asking one route in a [`RouteSnapshot`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteProbe {
    pub route: Route,
    pub local_address: Option<Ipv4Addr>,
    pub peer_address: Option<Ipv4Addr>,
    pub port: u16,
    pub up: bool,
    pub elapsed: Duration,
    pub error: Option<String>,
}

impl RouteProbe {
    pub fn local_socket(&self) -> Option<SocketAddrV4> {
        self.local_address
            .map(|address| SocketAddrV4::new(address, 0))
    }

    pub fn peer_socket(&self) -> Option<SocketAddrV4> {
        self.peer_address
            .map(|address| SocketAddrV4::new(address, self.port))
    }
}

/// A point-in-time, ordered view of the routes between two hosts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteSnapshot {
    pub local: Host,
    pub peer: Host,
    pub port: u16,
    pub elapsed: Duration,
    pub routes: Vec<RouteProbe>,
}

impl RouteSnapshot {
    /// The first available route in policy/input order.
    pub fn best(&self) -> Option<Route> {
        self.routes
            .iter()
            .find(|probe| probe.up)
            .map(|probe| probe.route)
    }

    pub fn route(&self, route: Route) -> Option<&RouteProbe> {
        self.routes.iter().find(|probe| probe.route == route)
    }

    pub fn available(&self) -> impl Iterator<Item = &RouteProbe> {
        self.routes.iter().filter(|probe| probe.up)
    }
}

/// Probe every route in the canonical connection-preference order.
pub fn probe(local: Host, peer: Host, port: u16) -> RouteSnapshot {
    probe_with_timeout(local, peer, port, PROBE)
}

/// Probe every route with a caller-selected connection timeout.
pub fn probe_with_timeout(local: Host, peer: Host, port: u16, timeout: Duration) -> RouteSnapshot {
    probe_routes_with_timeout(local, peer, port, &Route::every(), timeout)
}

/// Probe the requested routes concurrently while preserving their input order.
pub fn probe_routes(local: Host, peer: Host, port: u16, routes: &[Route]) -> RouteSnapshot {
    probe_routes_with_timeout(local, peer, port, routes, PROBE)
}

/// Probe selected routes with a caller-selected connection timeout.
pub fn probe_routes_with_timeout(
    local: Host,
    peer: Host,
    port: u16,
    routes: &[Route],
    timeout: Duration,
) -> RouteSnapshot {
    probe_routes_using(
        local,
        peer,
        port,
        routes,
        timeout,
        |host| host::lan_pair_with_timeout(host, timeout),
        |local, peer, timeout| socket::connect(Some(local), peer, timeout).map(drop),
    )
}

fn probe_routes_using<R, C>(
    local: Host,
    peer: Host,
    port: u16,
    routes: &[Route],
    timeout: Duration,
    resolve_lan: R,
    connect: C,
) -> RouteSnapshot
where
    R: Fn(Host) -> Result<(Ipv4Addr, Ipv4Addr), String> + Sync,
    C: Fn(Ipv4Addr, SocketAddrV4, Duration) -> io::Result<()> + Sync,
{
    let snapshot_started = Instant::now();
    let lan = OnceLock::new();
    let routes = std::thread::scope(|scope| {
        let workers = routes
            .iter()
            .copied()
            .map(|route| {
                let lan = &lan;
                let resolve_lan = &resolve_lan;
                let connect = &connect;
                (
                    route,
                    scope.spawn(move || {
                        let started = Instant::now();
                        let addresses = match route {
                            Route::Lan => addresses_from_lan(
                                local,
                                peer,
                                lan.get_or_init(|| resolve_lan(local)),
                            ),
                            _ => static_addresses(route, local, peer),
                        };
                        let (local_address, peer_address, up, error) = match addresses {
                            Ok((local_address, peer_address)) => {
                                let target = SocketAddrV4::new(peer_address, port);
                                let connected = connect(local_address, target, timeout);
                                match connected {
                                    Ok(()) => (Some(local_address), Some(peer_address), true, None),
                                    Err(error) => (
                                        Some(local_address),
                                        Some(peer_address),
                                        false,
                                        Some(error.to_string()),
                                    ),
                                }
                            }
                            Err(error) => (None, None, false, Some(error)),
                        };
                        RouteProbe {
                            route,
                            local_address,
                            peer_address,
                            port,
                            up,
                            elapsed: started.elapsed(),
                            error,
                        }
                    }),
                )
            })
            .collect::<Vec<_>>();

        // Joining in spawn order makes output deterministic without making the
        // probes sequential: all workers are already running at this point.
        workers
            .into_iter()
            .map(|(route, worker)| {
                worker.join().unwrap_or_else(|_| RouteProbe {
                    route,
                    local_address: None,
                    peer_address: None,
                    port,
                    up: false,
                    elapsed: snapshot_started.elapsed(),
                    error: Some("route probe worker panicked".into()),
                })
            })
            .collect()
    });
    RouteSnapshot {
        local,
        peer,
        port,
        elapsed: snapshot_started.elapsed(),
        routes,
    }
}

fn static_addresses(route: Route, local: Host, peer: Host) -> Result<(Ipv4Addr, Ipv4Addr), String> {
    debug_assert_ne!(route, Route::Lan);
    Ok((local.address(route)?, peer.address(route)?))
}

fn addresses_from_lan(
    local: Host,
    peer: Host,
    lan: &Result<(Ipv4Addr, Ipv4Addr), String>,
) -> Result<(Ipv4Addr, Ipv4Addr), String> {
    let &(local_lan, peer_lan) = lan.as_ref().map_err(Clone::clone)?;
    Ok((
        address_from_pair(local, local, local_lan, peer_lan),
        address_from_pair(peer, local, local_lan, peer_lan),
    ))
}

fn address_from_pair(host: Host, perspective: Host, local: Ipv4Addr, peer: Ipv4Addr) -> Ipv4Addr {
    if host == perspective { local } else { peer }
}

#[cfg(test)]
#[path = "../tests/unit/snapshot_tests.rs"]
mod tests;
