use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::host::{self, Host, PROBE, Route};
use crate::socket;

/// The result of asking one route in a [`RouteSnapshot`].
///
/// Resolution failures are kept in-band so a caller can render every route,
/// including a LAN that is not currently resolvable. Static routes always
/// carry both addresses; a failed dynamic LAN route carries neither.
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
///
/// LAN endpoint discovery is lazy and shared by every worker in this snapshot,
/// so it runs at most once. Its resolution time is included in the LAN probe's
/// elapsed time and overlaps the probes for static routes.
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
                                let remaining = timeout.saturating_sub(started.elapsed());
                                let connected = if remaining.is_zero() {
                                    Err(io::Error::new(
                                        io::ErrorKind::TimedOut,
                                        "route budget elapsed during address resolution",
                                    ))
                                } else {
                                    connect(local_address, target, remaining)
                                };
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
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex};

    const LOCAL_LAN: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 178);
    const PEER_LAN: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 162);

    fn refused() -> io::Error {
        io::Error::from(io::ErrorKind::ConnectionRefused)
    }

    #[test]
    fn canonical_snapshot_order_is_connection_preference_order() {
        let snapshot = probe_routes_using(
            Host::Macie,
            Host::Archie,
            22,
            &Route::every(),
            PROBE,
            |_| Ok((LOCAL_LAN, PEER_LAN)),
            |_, _, _| Err(refused()),
        );
        assert_eq!(
            snapshot
                .routes
                .iter()
                .map(|probe| probe.route)
                .collect::<Vec<_>>(),
            Route::every()
        );
    }

    #[test]
    fn requested_order_survives_out_of_order_completion() {
        let asked = [Route::Tailscale, Route::Cable, Route::Wifi];
        let snapshot = probe_routes_using(
            Host::Macie,
            Host::Archie,
            22,
            &asked,
            PROBE,
            |_| panic!("a snapshot without LAN must not resolve LAN"),
            |_, peer, _| {
                let delay = match peer.ip().octets()[2] {
                    231 => 5,
                    77 => 30,
                    _ => 15,
                };
                std::thread::sleep(Duration::from_millis(delay));
                Err(refused())
            },
        );
        assert_eq!(
            snapshot
                .routes
                .iter()
                .map(|probe| probe.route)
                .collect::<Vec<_>>(),
            asked
        );
    }

    #[test]
    fn lan_is_resolved_once_and_shared_by_both_endpoint_addresses() {
        let resolutions = AtomicUsize::new(0);
        let snapshot = probe_routes_using(
            Host::Macie,
            Host::Archie,
            22,
            &[Route::Lan, Route::Lan],
            PROBE,
            |_| {
                resolutions.fetch_add(1, Ordering::Relaxed);
                Ok((LOCAL_LAN, PEER_LAN))
            },
            |_, _, _| Err(refused()),
        );
        assert_eq!(resolutions.load(Ordering::Relaxed), 1);
        for probe in snapshot.routes {
            assert_eq!(probe.local_address, Some(LOCAL_LAN));
            assert_eq!(probe.peer_address, Some(PEER_LAN));
        }
    }

    #[test]
    fn lan_resolution_failure_is_a_visible_down_route() {
        let snapshot = probe_routes_using(
            Host::Macie,
            Host::Archie,
            22,
            &[Route::Lan],
            PROBE,
            |_| Err("mDNS unavailable".into()),
            |_, _, _| panic!("an unresolved route must not be connected"),
        );
        let lan = &snapshot.routes[0];
        assert!(!lan.up);
        assert_eq!(lan.local_address, None);
        assert_eq!(lan.peer_address, None);
        assert_eq!(lan.error.as_deref(), Some("mDNS unavailable"));
    }

    #[test]
    fn lan_resolution_and_connect_share_one_route_budget() {
        let connected = AtomicUsize::new(0);
        let budget = Duration::from_millis(15);
        let snapshot = probe_routes_using(
            Host::Macie,
            Host::Archie,
            22,
            &[Route::Lan],
            budget,
            |_| {
                std::thread::sleep(Duration::from_millis(20));
                Ok((LOCAL_LAN, PEER_LAN))
            },
            |_, _, _| {
                connected.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
        );
        assert_eq!(connected.load(Ordering::Relaxed), 0);
        assert!(!snapshot.routes[0].up);
        assert!(
            snapshot.routes[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("budget elapsed"))
        );
        assert!(snapshot.elapsed < Duration::from_millis(100));
    }

    #[test]
    fn all_selected_routes_are_in_flight_together() {
        // (currently active, total workers that have arrived)
        let state = Mutex::new((0usize, 0usize));
        let maximum = AtomicUsize::new(0);
        let changed = Condvar::new();
        let expected = 3;
        let snapshot = probe_routes_using(
            Host::Macie,
            Host::Archie,
            22,
            &[Route::Cable, Route::Wifi, Route::Tailscale],
            PROBE,
            |_| panic!("no LAN route was selected"),
            |_, _, _| {
                let mut state = state.lock().unwrap();
                state.0 += 1;
                state.1 += 1;
                maximum.fetch_max(state.0, Ordering::Relaxed);
                changed.notify_all();
                let (mut state, _) = changed
                    .wait_timeout_while(state, Duration::from_secs(1), |state| state.1 < expected)
                    .unwrap();
                state.0 -= 1;
                Err(refused())
            },
        );
        assert_eq!(snapshot.routes.len(), expected);
        assert_eq!(maximum.load(Ordering::Relaxed), expected);
    }

    #[test]
    fn availability_addresses_errors_and_elapsed_time_are_retained() {
        let snapshot = probe_routes_using(
            Host::Macie,
            Host::Archie,
            4242,
            &[Route::Cable, Route::Wifi],
            PROBE,
            |_| panic!("no LAN route was selected"),
            |_, peer, _| {
                std::thread::sleep(Duration::from_millis(10));
                if peer.ip().octets()[2] == 77 {
                    Ok(())
                } else {
                    Err(refused())
                }
            },
        );
        let cable = snapshot.route(Route::Cable).unwrap();
        assert!(cable.up);
        assert_eq!(cable.error, None);
        assert_eq!(cable.local_address, Some(Ipv4Addr::new(10, 77, 77, 1)));
        assert_eq!(cable.peer_address, Some(Ipv4Addr::new(10, 77, 77, 2)));
        assert_eq!(
            cable.peer_socket(),
            Some(SocketAddrV4::new(Ipv4Addr::new(10, 77, 77, 2), 4242))
        );
        assert!(cable.elapsed >= Duration::from_millis(10));

        let wifi = snapshot.route(Route::Wifi).unwrap();
        assert!(!wifi.up);
        assert!(wifi.error.as_deref().is_some_and(|error| !error.is_empty()));
        assert!(snapshot.elapsed >= cable.elapsed);
        assert_eq!(snapshot.available().count(), 1);
        assert_eq!(snapshot.best(), Some(Route::Cable));
    }

    #[test]
    fn best_uses_policy_order_not_completion_time() {
        let snapshot = probe_routes_using(
            Host::Macie,
            Host::Archie,
            22,
            &[Route::Cable, Route::Wifi, Route::Tailscale],
            PROBE,
            |_| panic!("no LAN route was selected"),
            |_, _, _| Ok(()),
        );
        assert_eq!(snapshot.best(), Some(Route::Cable));
    }

    #[test]
    fn a_panicking_worker_is_an_unavailable_route_not_a_snapshot_panic() {
        let snapshot = probe_routes_using(
            Host::Macie,
            Host::Archie,
            22,
            &[Route::Cable],
            PROBE,
            |_| panic!("no LAN route was selected"),
            |_, _, _| panic!("synthetic connector panic"),
        );
        let cable = &snapshot.routes[0];
        assert!(!cable.up);
        assert_eq!(cable.error.as_deref(), Some("route probe worker panicked"));
    }

    #[test]
    fn an_empty_selection_is_a_valid_instant_snapshot() {
        let snapshot = probe_routes_using(
            Host::Macie,
            Host::Archie,
            22,
            &[],
            PROBE,
            |_| panic!("no routes means no resolution"),
            |_, _, _| panic!("no routes means no connections"),
        );
        assert!(snapshot.routes.is_empty());
        assert_eq!(snapshot.best(), None);
    }
}
