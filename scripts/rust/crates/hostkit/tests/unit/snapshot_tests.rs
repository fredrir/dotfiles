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
fn lan_resolution_does_not_consume_the_connection_budget() {
    let connected = AtomicUsize::new(0);
    let budget = Duration::from_millis(15);
    let snapshot = probe_routes_using(
        Host::Macie,
        Host::Archie,
        22,
        &[Route::Lan],
        budget,
        |_| {
            std::thread::sleep(Duration::from_millis(10));
            Ok((LOCAL_LAN, PEER_LAN))
        },
        |_, _, timeout| {
            assert_eq!(timeout, budget);
            connected.fetch_add(1, Ordering::Relaxed);
            Ok(())
        },
    );
    assert_eq!(connected.load(Ordering::Relaxed), 1);
    assert!(snapshot.routes[0].up);
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
