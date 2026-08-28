//! Which routes the mux has a domain for, and what those domains are called.

use hostkit::{Host, Route};

/// The routes `tls.lua` builds a client domain for, in the order `ssh` prefers
/// and without the `Route::Lan` that no wezterm config defines a domain for.
pub const ROUTES: [Route; 3] = [Route::Cable, Route::Wifi, Route::Tailscale];

/// The client domain `tls.lua` names for this peer and route.
pub fn name(peer: Host, route: Route) -> String {
    format!("{}-{}", peer.name(), route.name())
}

/// The machine to probe, which is this one's peer unless another was named.
pub fn target(named: Option<&str>, this: Host) -> Result<Host, String> {
    let Some(named) = named else {
        return Ok(this.peer());
    };
    let host = Host::from_name(named)?;
    if host == this {
        return Err(format!(
            "{named} is this machine; its panes are already in localmux"
        ));
    }
    Ok(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mux_routes_are_the_ssh_order_without_the_lan() {
        let expected: Vec<Route> = Route::every()
            .into_iter()
            .filter(|route| *route != Route::Lan)
            .collect();
        assert_eq!(ROUTES.to_vec(), expected);
        assert_eq!(ROUTES[0], Route::Cable);
        assert_eq!(ROUTES[2], Route::Tailscale);
    }

    #[test]
    fn no_mux_route_is_the_lan() {
        assert!(!ROUTES.contains(&Route::Lan));
        assert!(Route::every().contains(&Route::Lan));
    }

    #[test]
    fn a_domain_is_spelled_the_way_tls_lua_spells_it() {
        assert_eq!(name(Host::Archie, Route::Cable), "archie-cable");
        assert_eq!(name(Host::Archie, Route::Wifi), "archie-wifi");
        assert_eq!(name(Host::Archie, Route::Tailscale), "archie-tailscale");
        assert_eq!(name(Host::Macie, Route::Cable), "macie-cable");
        assert_eq!(name(Host::Macie, Route::Tailscale), "macie-tailscale");
    }

    #[test]
    fn nothing_named_means_the_peer() {
        assert_eq!(target(None, Host::Macie).unwrap(), Host::Archie);
        assert_eq!(target(None, Host::Archie).unwrap(), Host::Macie);
        assert_eq!(target(Some("archie"), Host::Macie).unwrap(), Host::Archie);
    }

    #[test]
    fn this_machine_has_no_domain_pointing_at_itself() {
        let refused = target(Some("macie"), Host::Macie).unwrap_err();
        assert!(refused.contains("this machine"), "{refused}");
        assert!(target(Some("nowhere"), Host::Macie).is_err());
    }
}
