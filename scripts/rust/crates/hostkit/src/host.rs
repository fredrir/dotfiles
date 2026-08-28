
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use clap::ValueEnum;

use crate::socket;

pub const PROBE: Duration = Duration::from_millis(400);

const PROBE_PORT: u16 = 22;

pub const MUX_PORT: u16 = 8443;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    Macie,
    Archie,
}

impl Host {
    pub fn this() -> Result<Host, String> {
        match std::env::consts::OS {
            "macos" => Ok(Host::Macie),
            "linux" => Ok(Host::Archie),
            other => Err(format!("unsupported operating system: {other}")),
        }
    }

    pub fn peer(self) -> Host {
        match self {
            Host::Macie => Host::Archie,
            Host::Archie => Host::Macie,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Host::Macie => "macie",
            Host::Archie => "archie",
        }
    }

    pub fn from_name(name: &str) -> Result<Host, String> {
        [Host::Macie, Host::Archie]
            .into_iter()
            .find(|host| host.name() == name)
            .ok_or_else(|| format!("unknown host: {name} (macie or archie)"))
    }

    pub fn address(self, route: Route) -> Result<Ipv4Addr, String> {
        match (route, self) {
            (Route::Cable, Host::Macie) => Ok(Ipv4Addr::new(10, 77, 77, 1)),
            (Route::Cable, Host::Archie) => Ok(Ipv4Addr::new(10, 77, 77, 2)),
            (Route::Wifi, Host::Macie) => Ok(Ipv4Addr::new(10, 77, 78, 1)),
            (Route::Wifi, Host::Archie) => Ok(Ipv4Addr::new(10, 77, 78, 2)),
            (Route::Tailscale, Host::Macie) => Ok(Ipv4Addr::new(100, 75, 71, 79)),
            (Route::Tailscale, Host::Archie) => Ok(Ipv4Addr::new(100, 126, 231, 24)),
            (Route::Lan, host) => {
                let this = Host::this()?;
                let (local, peer) = lan_pair(this)?;
                Ok(if host == this { local } else { peer })
            }
        }
    }

    fn lan_name(self) -> &'static str {
        match self {
            Host::Macie => "macie-2.local",
            Host::Archie => "archpc.local",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Route {
    #[value(alias = "usb")]
    Cable,
    #[value(alias = "direct", alias = "wireless")]
    Wifi,
    Lan,
    #[value(alias = "ts")]
    Tailscale,
}

impl Route {
    pub fn name(self) -> &'static str {
        match self {
            Route::Cable => "cable",
            Route::Wifi => "wifi",
            Route::Lan => "lan",
            Route::Tailscale => "tailscale",
        }
    }

    pub fn every() -> [Route; 4] {
        [Route::Cable, Route::Wifi, Route::Lan, Route::Tailscale]
    }

    pub fn up(self, this: Host) -> bool {
        self.answers(this, this.peer(), PROBE_PORT)
    }

    pub fn answers(self, from: Host, to: Host, port: u16) -> bool {
        let (Ok(local), Ok(peer)) = (from.address(self), to.address(self)) else {
            return false;
        };
        socket::connect(Some(local), std::net::SocketAddrV4::new(peer, port), PROBE).is_ok()
    }
}

fn lan_pair(this: Host) -> Result<(Ipv4Addr, Ipv4Addr), String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    let helper = PathBuf::from(home).join(".ssh/bin/home-lan-connect");
    let output = Command::new(&helper)
        .args(["--resolve", this.peer().lan_name()])
        .output()
        .map_err(|error| format!("{}: {error}", helper.display()))?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if reason.is_empty() {
            "regular LAN is not resolvable on 192.168.1.0/24".into()
        } else {
            reason
        });
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "home-lan-connect returned non-UTF-8 output".to_string())?;
    parse_lan_pair(&text)
}

fn parse_lan_pair(text: &str) -> Result<(Ipv4Addr, Ipv4Addr), String> {
    let mut fields = text.split_whitespace();
    let local = fields
        .next()
        .ok_or_else(|| "home-lan-connect returned no local address".to_string())?
        .parse()
        .map_err(|_| "home-lan-connect returned an invalid local address".to_string())?;
    let peer = fields
        .next()
        .ok_or_else(|| "home-lan-connect returned no peer address".to_string())?
        .parse()
        .map_err(|_| "home-lan-connect returned an invalid peer address".to_string())?;
    if fields.next().is_some() {
        return Err("home-lan-connect returned too many fields".into());
    }
    Ok((local, peer))
}

pub fn best(this: Host) -> Option<Route> {
    Route::every().into_iter().find(|route| route.up(this))
}

#[cfg(test)]
mod tests {
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
}
