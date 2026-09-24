use super::*;

#[test]
fn a_route_earlier_in_the_ssh_order_is_better() {
    assert!(better(Some(Route::Cable), Some(Route::Tailscale)));
    assert!(better(Some(Route::Wifi), Some(Route::Lan)));
}

#[test]
fn the_same_or_a_worse_route_is_not_worth_reconnecting_for() {
    assert!(!better(Some(Route::Lan), Some(Route::Lan)));
    assert!(!better(Some(Route::Tailscale), Some(Route::Cable)));
}

#[test]
fn an_unknown_route_never_triggers_a_reconnect() {
    assert!(!better(None, Some(Route::Tailscale)));
    assert!(!better(Some(Route::Cable), None));
}

fn listener(port: u16, address: &str, pid: u32) -> Listener {
    Listener {
        port,
        address: address.parse().unwrap(),
        process: format!("pid{pid}"),
        pid,
    }
}

#[test]
fn only_a_wildcard_listener_holds_the_port_against_the_alias() {
    let local = [listener(5173, "127.0.0.1", 7), listener(8080, "0.0.0.0", 8)];
    assert_eq!(holder(&local, 5173, 1, wildcard), None);
    assert_eq!(holder(&local, 8080, 1, wildcard).as_deref(), Some("pid8"));
}

#[test]
fn the_masters_own_sockets_never_count_as_a_holder() {
    let local = [listener(5173, "127.0.0.1", 42)];
    assert_eq!(
        holder(&local, 5173, 42, Listener::reachable_through_loopback),
        None
    );
    assert_eq!(
        holder(&local, 5173, 1, Listener::reachable_through_loopback).as_deref(),
        Some("pid42")
    );
}
