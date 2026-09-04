use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{io::Read, thread};

use clap::ValueEnum;

use crate::socket;

pub const PROBE: Duration = Duration::from_secs(1);

const PROBE_PORT: u16 = 22;

pub const MUX_PORT: u16 = 8443;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
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
            Host::Archie => "archie.local",
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
        let addresses = match self {
            Route::Lan => lan_addresses(from, to),
            _ => from
                .address(self)
                .and_then(|local| to.address(self).map(|peer| (local, peer))),
        };
        let Ok((local, peer)) = addresses else {
            return false;
        };
        socket::connect(Some(local), std::net::SocketAddrV4::new(peer, port), PROBE).is_ok()
    }
}

fn lan_addresses(from: Host, to: Host) -> Result<(Ipv4Addr, Ipv4Addr), String> {
    let perspective = Host::this()?;
    let (local, peer) = lan_pair(perspective)?;
    let address = |host| if host == perspective { local } else { peer };
    Ok((address(from), address(to)))
}

pub(crate) fn lan_pair(this: Host) -> Result<(Ipv4Addr, Ipv4Addr), String> {
    lan_pair_with_timeout(this, PROBE)
}

pub(crate) fn lan_pair_with_timeout(
    this: Host,
    timeout: Duration,
) -> Result<(Ipv4Addr, Ipv4Addr), String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    let helper = PathBuf::from(home).join(".ssh/bin/home-lan-connect");
    let mut child = Command::new(&helper)
        .args(["--resolve", this.peer().lan_name()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{}: {error}", helper.display()))?;
    let started = Instant::now();
    let status = loop {
        match child
            .try_wait()
            .map_err(|error| format!("{}: {error}", helper.display()))?
        {
            Some(status) => break status,
            None if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "regular LAN resolution exceeded {:.0} ms",
                    timeout.as_secs_f64() * 1_000.0
                ));
            }
            None => thread::sleep(Duration::from_millis(5)),
        }
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_end(&mut stdout)
            .map_err(|error| format!("{}: {error}", helper.display()))?;
    }
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_end(&mut stderr)
            .map_err(|error| format!("{}: {error}", helper.display()))?;
    }
    if !status.success() {
        let reason = String::from_utf8_lossy(&stderr).trim().to_string();
        return Err(if reason.is_empty() {
            "regular LAN is not resolvable on 192.168.1.0/24".into()
        } else {
            reason
        });
    }
    let text = String::from_utf8(stdout)
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
    best_using(this, crate::ssh::resolved)
}

fn best_using(this: Host, resolve: impl FnOnce(&str) -> Option<Route>) -> Option<Route> {
    resolve(this.peer().name())
}

#[cfg(test)]
#[path = "../tests/unit/host_tests.rs"]
mod tests;
