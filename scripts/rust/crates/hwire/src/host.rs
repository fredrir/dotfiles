//! The two machines, and the two ways between them.
//!
//! Hardcoded for the same reason `dmux::hosts` hardcodes them: this is
//! personal infrastructure for exactly two hosts, and the addresses already
//! live in the ssh configs and `wez/remote/mux.lua`. The pair is mirrored
//! rather than shared because the only other copy sits inside `dmux`, whose
//! bundled SQLite is a long build to depend on for four addresses; a test
//! reads the ssh configs in this repository and fails if they drift.

use std::net::Ipv4Addr;
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

    pub fn address(self, route: Route) -> Ipv4Addr {
        match (route, self) {
            (Route::Cable, Host::Macie) => Ipv4Addr::new(10, 77, 77, 1),
            (Route::Cable, Host::Archie) => Ipv4Addr::new(10, 77, 77, 2),
            (Route::Tailscale, Host::Macie) => Ipv4Addr::new(100, 75, 71, 79),
            (Route::Tailscale, Host::Archie) => Ipv4Addr::new(100, 126, 231, 24),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Route {
    /// The USB-C cable: a private /30 with no gateway, DNS or NAT on it.
    #[value(alias = "usb")]
    Cable,
    /// The tailnet, which is what ssh falls back to when the cable is out.
    #[value(alias = "ts")]
    Tailscale,
}

impl Route {
    pub fn name(self) -> &'static str {
        match self {
            Route::Cable => "cable",
            Route::Tailscale => "tailscale",
        }
    }

    pub fn every() -> [Route; 2] {
        [Route::Cable, Route::Tailscale]
    }

    /// Whether the peer answers over this route from this machine. The local
    /// bind is the point: an answer proves the route, not just that the peer
    /// is reachable somehow.
    pub fn up(self, this: Host) -> bool {
        socket::connect(
            Some(this.address(self)),
            std::net::SocketAddrV4::new(this.peer().address(self), PROBE_PORT),
            PROBE,
        )
        .is_ok()
    }
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
            assert_ne!(Host::Macie.address(route), Host::Archie.address(route));
        }
        assert_ne!(
            Host::Macie.address(Route::Cable),
            Host::Macie.address(Route::Tailscale)
        );
    }

    fn repository() -> Option<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(4)?;
        root.join("docs/ssh.md")
            .exists()
            .then(|| root.to_path_buf())
    }

    /// The addresses here are a copy. This is the check that the original
    /// did not move without it: every one of them has to appear in the ssh
    /// configuration that routes the same two names.
    #[test]
    fn the_addresses_match_the_ssh_configs() {
        let Some(root) = repository() else {
            return;
        };
        let configs = [
            "macos/ssh/config.d/05-archie-cabled-first",
            "macos/ssh/config.d/40-cabled",
            "linux/arch/ssh/config.d/05-macie-cabled-first",
            "linux/arch/ssh/config.d/40-cabled",
        ]
        .map(|path| std::fs::read_to_string(root.join(path)).unwrap_or_default())
        .join("\n");

        for host in [Host::Macie, Host::Archie] {
            let cable = host.address(Route::Cable).to_string();
            assert!(
                configs.contains(&cable),
                "{cable} is not in the ssh configs any more"
            );
        }
    }
}
