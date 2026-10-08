use super::*;

#[test]
fn a_pane_stamp_names_its_origin_and_route() {
    let stamp = parse("v1:macie:archie:cable:tls", Host::Archie).unwrap();
    assert_eq!(
        stamp,
        Stamp {
            origin: Host::Macie,
            destination: Host::Archie,
            route: Route::Cable
        }
    );
}

#[test]
fn every_mux_route_is_a_tls_route_including_the_lan() {
    for route in Route::every() {
        let text = format!("v1:archie:macie:{}:tls", route.name());
        assert_eq!(parse(&text, Host::Macie).unwrap().route, route);
    }
}

#[test]
fn a_stamp_is_bound_to_the_process_it_lands_in() {
    let refused = parse("v1:archie:macie:cable:tls", Host::Archie).unwrap_err();
    assert!(refused.contains("archie --> macie"), "{refused}");
    assert!(parse("v1:archie:archie:cable:tls", Host::Archie).is_err());
}

#[test]
fn a_malformed_stamp_is_refused() {
    for text in [
        "",
        "anything",
        "v1:macie:archie:cable",
        "v2:macie:archie:cable:tls",
        "v1:macie:archie:cable:ssh",
        "v1:macie:archie:cable:tls:extra",
        "v1:nowhere:archie:cable:tls",
        "v1:macie:archie:pigeon:tls",
    ] {
        assert!(parse(text, Host::Archie).is_err(), "{text}");
    }
}
