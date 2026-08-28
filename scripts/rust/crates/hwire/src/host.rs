//! The two machines, and the four ways between them.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use clap::ValueEnum;

use crate::socket;

/// How long a route probe waits before calling the route down. The cable
/// answers in under a millisecond and Tailscale in a few, so this only ever
/// elapses when nothing is there.
pub const PROBE: Duration = Duration::from_millis(400);

/// The port both routes are probed on: sshd is the one service guaranteed to
/// be listening on both machines, and reaching it is what `ssh` needs anyway.
const PROBE_PORT: u16 = 22;

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

    /// Also the ssh host name, which is how the peer's half gets started.
    pub fn name(self) -> &'static str {
        match self {
            Host::Macie => "macie",
            Host::Archie => "archie",
        }
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
    /// The USB-C cable: a private /30 with no gateway, DNS or NAT on it.
    #[value(alias = "usb")]
    Cable,
    /// Macie associated directly to Archie's private Wi-Fi AP.
    #[value(alias = "direct", alias = "wireless")]
    Wifi,
    /// The home 192.168.1.0/24, resolved through filtered mDNS.
    Lan,
    /// The tailnet, which is what ssh falls back to when the cable is out.
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

    /// Whether the peer answers over this route from this machine. The local
    /// bind is the point: an answer proves the route, not just that the peer
    /// is reachable somehow.
    pub fn up(self, this: Host) -> bool {
        let (Ok(local), Ok(peer)) = (this.address(self), this.peer().address(self)) else {
            return false;
        };
        socket::connect(
            Some(local),
            std::net::SocketAddrV4::new(peer, PROBE_PORT),
            PROBE,
        )
        .is_ok()
    }
}

/// Ask the same route-pinning helper OpenSSH uses for the local and peer LAN
/// addresses. It rejects a result unless both are on 192.168.1.0/24.
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

/// Which route to measure when none was named: the cable when it is there,
/// which is the same order `ssh` resolves `archie` and `macie` in.
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

    /// Every file outside this crate that hard-codes the cable's addresses:
    /// the four ssh files that route the two names, and the wezterm config
    /// that probes the same link.
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
        "shared/wezterm/wez/remote/mux.lua",
    ];

    fn routed(root: &Path) -> impl Iterator<Item = (&'static str, String)> + '_ {
        ROUTED.into_iter().map(|path| {
            (
                path,
                std::fs::read_to_string(root.join(path)).unwrap_or_default(),
            )
        })
    }

    /// The addresses here are a copy. This is the check that the original
    /// did not move without it: every one of them has to appear in the
    /// configuration that routes the same two names.
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

    /// Neither end of the cable has a stable interface name -- macOS renumbers
    /// `enN` whenever the NCM MAC changes, and archie's `macie0` exists only
    /// because a .link file mints it. Binding a name instead of an address
    /// fails the probe rather than the cable, so every connection falls back
    /// to Tailscale without saying so. The addresses cannot drift that way,
    /// which is why the check above is worth having and this one keeps it so.
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
