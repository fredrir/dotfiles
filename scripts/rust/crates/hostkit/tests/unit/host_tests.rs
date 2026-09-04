use super::*;
use std::path::{Path, PathBuf};

#[test]
fn the_peers_are_symmetric() {
    assert_eq!(Host::Macie.peer(), Host::Archie);
    assert_eq!(Host::Archie.peer(), Host::Macie);
    assert_eq!(Host::Archie.peer().peer(), Host::Archie);
}

#[test]
fn a_host_name_round_trips() {
    for host in [Host::Macie, Host::Archie] {
        assert_eq!(Host::from_name(host.name()).unwrap(), host);
    }
    assert!(Host::from_name("Macie").is_err());
    assert!(Host::from_name("").is_err());
}

#[test]
fn the_best_route_is_the_one_openssh_resolves() {
    let route = best_using(Host::Macie, |name| {
        assert_eq!(name, "archie");
        Some(Route::Lan)
    });
    assert_eq!(route, Some(Route::Lan));
}

#[test]
fn a_route_gives_each_machine_its_own_address() {
    for route in Route::every() {
        // LAN discovery is deliberately live and may be absent in a
        // hermetic test; its parser is covered by the connector tests.
        if route != Route::Lan {
            assert_ne!(
                Host::Macie.address(route).unwrap(),
                Host::Archie.address(route).unwrap()
            );
        }
    }
    assert_ne!(
        Host::Macie.address(Route::Cable).unwrap(),
        Host::Macie.address(Route::Tailscale).unwrap()
    );
}

fn repository() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(4)?;
    root.join("docs/ssh.md")
        .exists()
        .then(|| root.to_path_buf())
}

const ROUTED: [&str; 10] = [
    "macos/ssh/config.d/05-archie-cabled-first",
    "macos/ssh/config.d/06-archie-wifi-first",
    "macos/ssh/config.d/07-archie-lan-first",
    "macos/ssh/config.d/40-cabled",
    "linux/arch/ssh/config.d/05-macie-cabled-first",
    "linux/arch/ssh/config.d/06-macie-wifi-first",
    "linux/arch/ssh/config.d/07-macie-lan-first",
    "linux/arch/ssh/config.d/40-cabled",
    "shared/ssh/bin/home-lan-connect",
    "shared/wezterm/domain/hosts.lua",
];

#[test]
fn the_repository_anchor_resolves() {
    assert!(
        repository().is_some(),
        "repository() found no repo root from {}, so the drift guards are covering nothing",
        env!("CARGO_MANIFEST_DIR")
    );
}

fn routed(root: &Path) -> impl Iterator<Item = (&'static str, String)> + '_ {
    ROUTED.into_iter().map(|path| {
        let text = std::fs::read_to_string(root.join(path))
            .unwrap_or_else(|error| panic!("{path} is in ROUTED but unreadable: {error}"));
        (path, text)
    })
}

#[test]
fn the_addresses_match_the_ssh_configs() {
    let Some(root) = repository() else {
        return;
    };
    let configs = routed(&root)
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n");

    for host in [Host::Macie, Host::Archie] {
        let cable = host.address(Route::Cable).unwrap().to_string();
        assert!(
            configs.contains(&cable),
            "{cable} is not in the ssh configs any more"
        );
    }
}

#[test]
fn the_mux_port_and_route_names_match_hosts_lua() {
    let Some(root) = repository() else {
        return;
    };
    let hosts = std::fs::read_to_string(root.join("shared/wezterm/domain/hosts.lua")).unwrap();
    assert!(
        hosts.contains(&format!("local port = {MUX_PORT}")),
        "hosts.lua no longer says port {MUX_PORT}"
    );
    let mut routes = 0;
    for route in Route::every() {
        if route == Route::Lan {
            continue;
        }
        routes += 1;
        let name = format!("name = {:?}", route.name());
        assert!(hosts.contains(&name), "hosts.lua has no route {name}");

        // The ssh guard only proves an address is somewhere in ten files;
        // this proves hosts.lua itself carries the one that gets dialled.
        for host in [Host::Macie, Host::Archie] {
            let address = format!("address = {:?}", host.address(route).unwrap().to_string());
            assert!(hosts.contains(&address), "hosts.lua has no {address}");
        }
    }

    // The checks above are one-directional, so a route added to hosts.lua
    // would build a wezterm domain this table never offers.
    let listed = hosts.matches("{ name = ").count();
    assert_eq!(
        listed,
        routes * 2,
        "hosts.lua lists {listed} routes across the two hosts, this table has {}",
        routes * 2
    );
}

#[test]
fn route_order_matches_ssh_policy() {
    assert_eq!(
        Route::every(),
        [Route::Cable, Route::Wifi, Route::Lan, Route::Tailscale]
    );
}

#[test]
fn filtered_lan_pair_has_exactly_two_ipv4_addresses() {
    assert_eq!(
        parse_lan_pair("192.168.1.178 192.168.1.162\n").unwrap(),
        (
            Ipv4Addr::new(192, 168, 1, 178),
            Ipv4Addr::new(192, 168, 1, 162)
        )
    );
    assert!(parse_lan_pair("").is_err());
    assert!(parse_lan_pair("192.168.1.178 nope").is_err());
    assert!(parse_lan_pair("192.168.1.178 192.168.1.162 extra").is_err());
}

#[test]
fn nothing_binds_an_interface_name() {
    let Some(root) = repository() else {
        return;
    };
    for (path, text) in routed(&root) {
        assert!(
            !text.contains("BindInterface"),
            "{path} binds an interface name; bind the address instead"
        );
        assert!(
            !text.contains("-b ") && !text.contains("'-b'"),
            "{path} probes with nc -b; bind this end's address with -s instead"
        );
    }
}
