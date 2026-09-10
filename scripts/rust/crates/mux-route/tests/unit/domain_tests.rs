use super::*;

#[test]
fn the_mux_routes_are_the_ssh_order() {
    assert_eq!(ROUTES, Route::every());
    assert_eq!(ROUTES[0], Route::Cable);
    assert_eq!(ROUTES[3], Route::Tailscale);
}

#[test]
fn the_lan_is_a_mux_route_ranked_above_tailscale() {
    assert_eq!(ROUTES[2], Route::Lan);
    let lan = ROUTES.iter().position(|route| *route == Route::Lan);
    let tailscale = ROUTES.iter().position(|route| *route == Route::Tailscale);
    assert!(lan < tailscale);
}

#[test]
fn a_domain_is_spelled_the_way_tls_lua_spells_it() {
    assert_eq!(name(Host::Archie, Route::Cable), "archie-cable");
    assert_eq!(name(Host::Archie, Route::Wifi), "archie-wifi");
    assert_eq!(name(Host::Archie, Route::Lan), "archie-lan");
    assert_eq!(name(Host::Archie, Route::Tailscale), "archie-tailscale");
    assert_eq!(name(Host::Macie, Route::Cable), "macie-cable");
    assert_eq!(name(Host::Macie, Route::Lan), "macie-lan");
    assert_eq!(name(Host::Macie, Route::Tailscale), "macie-tailscale");
}

#[test]
fn nothing_named_means_the_peer() {
    assert_eq!(target(None, Host::Macie).unwrap(), Host::Archie);
    assert_eq!(target(None, Host::Archie).unwrap(), Host::Macie);
    assert_eq!(
        target(Some(Host::Archie), Host::Macie).unwrap(),
        Host::Archie
    );
}

#[test]
fn this_machine_has_no_domain_pointing_at_itself() {
    let refused = target(Some(Host::Macie), Host::Macie).unwrap_err();
    assert!(refused.contains("macie"), "{refused}");
    assert!(refused.contains("this machine"), "{refused}");
}
