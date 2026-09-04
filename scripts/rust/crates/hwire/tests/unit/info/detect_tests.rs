use super::*;

#[test]
fn ssh_addresses_identify_every_fixed_route() {
    let cases = [
        ("10.77.77.2", Route::Cable),
        ("10.77.78.2", Route::Wifi),
        ("100.126.231.24", Route::Tailscale),
        ("192.168.1.162", Route::Lan),
    ];
    for (server, route) in cases {
        let found = parse_ssh(&format!("192.0.2.1 54321 {server} 22"), Host::Archie).unwrap();
        assert_eq!(found.route, Some(route));
        assert_eq!(found.from, Host::Macie);
        assert_eq!(found.to, Host::Archie);
        assert!(!found.tls);
    }
}

#[test]
fn an_unknown_ssh_address_stays_unknown() {
    let found = parse_ssh("203.0.113.5 1 172.16.0.2 22", Host::Archie).unwrap();
    assert_eq!(found.route, None);
}

#[test]
fn tls_stamps_are_strict_and_host_bound() {
    for (this, peer) in [(Host::Archie, Host::Macie), (Host::Macie, Host::Archie)] {
        let stamp = format!("v1:{}:{}:cable:tls", peer.name(), this.name());
        let found = parse_tls(&stamp, this).unwrap();
        assert_eq!(found.route, Some(Route::Cable));
        assert!(found.tls);
        assert_eq!(
            found.domain.as_deref(),
            Some(format!("{}-cable", this.name()).as_str())
        );
    }
    assert!(parse_tls("v1:archie:macie:cable:tls", Host::Archie).is_err());
    assert!(parse_tls("v1:macie:archie:lan:tls", Host::Archie).is_err());
    assert!(parse_tls("anything", Host::Archie).is_err());
}

#[test]
fn malformed_ssh_state_is_rejected() {
    assert!(parse_ssh("", Host::Macie).is_err());
    assert!(parse_ssh("bad 1 10.77.77.1 22", Host::Macie).is_err());
    assert!(parse_ssh("1.2.3.4 1 10.77.77.1 22 extra", Host::Macie).is_err());
}
