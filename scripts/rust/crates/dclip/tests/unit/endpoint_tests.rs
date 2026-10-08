use super::*;

fn at(text: &str) -> SocketAddrV4 {
    text.parse().unwrap()
}

#[test]
fn archie_binds_its_route_addresses_and_the_lan_relay_target() {
    assert_eq!(listen(Host::Archie, Route::Cable), at("10.77.77.2:8453"));
    assert_eq!(listen(Host::Archie, Route::Wifi), at("10.77.78.2:8453"));
    assert_eq!(
        listen(Host::Archie, Route::Tailscale),
        at("100.124.205.100:8453")
    );
    assert_eq!(listen(Host::Archie, Route::Lan), at("127.0.0.1:8457"));
}

#[test]
fn macie_binds_loopback_ports_behind_its_relays() {
    assert_eq!(listen(Host::Macie, Route::Cable), at("127.0.0.1:8453"));
    assert_eq!(listen(Host::Macie, Route::Wifi), at("127.0.0.1:8454"));
    assert_eq!(listen(Host::Macie, Route::Tailscale), at("127.0.0.1:8456"));
    assert_eq!(listen(Host::Macie, Route::Lan), at("127.0.0.1:8457"));
}

#[test]
fn every_bind_on_one_machine_is_distinct() {
    for host in [Host::Macie, Host::Archie] {
        let mut addresses: Vec<_> = Route::every().map(|route| listen(host, route)).to_vec();
        addresses.sort();
        addresses.dedup();
        assert_eq!(addresses.len(), 4);
    }
}

#[test]
fn a_client_dials_the_peer_route_address_or_the_local_lan_relay() {
    assert_eq!(dial(Host::Macie, Route::Cable), at("10.77.77.1:8453"));
    assert_eq!(dial(Host::Macie, Route::Tailscale), at("100.75.71.79:8453"));
    assert_eq!(dial(Host::Archie, Route::Wifi), at("10.77.78.2:8453"));
    assert_eq!(dial(Host::Archie, Route::Lan), at("127.0.0.1:8458"));
    assert_eq!(dial(Host::Macie, Route::Lan), at("127.0.0.1:8458"));
}
