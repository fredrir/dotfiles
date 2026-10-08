use super::*;

#[test]
fn a_tls_pane_on_archie_points_back_at_macie() {
    let stamp = parse("v1:macie:archie:cable:tls", Host::Archie).unwrap();
    assert_eq!(
        stamp,
        Stamp {
            origin: Host::Macie,
            route: Route::Cable
        }
    );
}

#[test]
fn every_mux_route_is_accepted_including_the_lan() {
    for route in Route::every() {
        let text = format!("v1:archie:macie:{}:tls", route.name());
        assert_eq!(parse(&text, Host::Macie).unwrap().route, route);
    }
}

#[test]
fn a_stamp_for_another_direction_is_refused() {
    let refused = parse("v1:archie:macie:cable:tls", Host::Archie).unwrap_err();
    assert!(refused.contains("archie --> macie"), "{refused}");
    assert!(parse("v1:archie:archie:cable:tls", Host::Archie).is_err());
}

#[test]
fn a_malformed_stamp_is_refused() {
    for text in [
        "",
        "v1:macie:archie:cable",
        "v2:macie:archie:cable:tls",
        "v1:macie:archie:cable:ssh",
        "v1:macie:archie:cable:tls:extra",
        "v1:nowhere:archie:cable:tls",
    ] {
        assert!(parse(text, Host::Archie).is_err(), "{text}");
    }
}

#[test]
fn route_aliases_are_not_part_of_the_stamp() {
    assert!(parse("v1:macie:archie:usb:tls", Host::Archie).is_err());
    assert_eq!(route_named("tailscale"), Some(Route::Tailscale));
    assert_eq!(route_named("ts"), None);
}
