use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use hostkit::{Host, Route};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Context {
    Local,
    Ssh,
    Tls,
    Query,
}

impl Context {
    pub fn name(self) -> &'static str {
        match self {
            Context::Local => "local",
            Context::Ssh => "ssh",
            Context::Tls => "tls",
            Context::Query => "query",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Session {
    pub from: Host,
    pub to: Host,
    pub route: Option<Route>,
    pub tls: bool,
    pub client_address: Option<IpAddr>,
    pub client_port: Option<u16>,
    pub server_address: Option<IpAddr>,
    pub server_port: Option<u16>,
    pub domain: Option<String>,
    pub evidence: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteState {
    pub route: Route,
    pub local: Option<Ipv4Addr>,
    pub peer: Option<Ipv4Addr>,
    pub available: bool,
    pub elapsed: Duration,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MasterInfo {
    pub running: bool,
    pub control_path: Option<String>,
    pub age: Option<Duration>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TargetInfo {
    pub input: String,
    pub hostname: String,
    pub route: Option<Route>,
    pub bound: Option<String>,
    pub proxy: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub master: MasterInfo,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub context: Context,
    pub this: Host,
    pub peer: Host,
    pub session: Option<Session>,
    pub preferred: Option<Route>,
    pub routes: Vec<RouteState>,
    pub targets: Vec<TargetInfo>,
    pub warnings: Vec<String>,
}

impl Snapshot {
    pub fn fingerprint(&self) -> String {
        let mut value = format!(
            "{}:{:?}:{:?}:",
            self.context.name(),
            self.preferred,
            self.session.as_ref().map(|session| {
                (
                    session.route,
                    session.tls,
                    session.from,
                    session.to,
                    session.client_address,
                    session.client_port,
                    session.server_address,
                    session.server_port,
                    session.domain.as_deref(),
                    session.evidence,
                )
            })
        );
        for route in &self.routes {
            value.push_str(&format!(
                "{}{}:{:?}:{:?}:{:?};",
                route.route.name(),
                u8::from(route.available),
                route.local,
                route.peer,
                route.error,
            ));
        }
        for target in &self.targets {
            value.push_str(&format!(
                ":{}:{}:{:?}:{:?}:{:?}:{:?}:{:?}:{}:{:?}:{:?}:{:?}",
                target.input,
                target.hostname,
                target.route,
                target.bound,
                target.proxy,
                target.user,
                target.port,
                target.master.running,
                target.master.control_path,
                target.master.detail,
                target.error,
            ));
        }
        for warning in &self.warnings {
            value.push_str(warning);
        }
        value
    }

    pub fn primary_route(&self) -> Option<Route> {
        self.preferred
            .or_else(|| self.session.as_ref().and_then(|session| session.route))
            .or_else(|| self.targets.first().and_then(|target| target.route))
    }

    pub fn failure(&self) -> Option<String> {
        let failed = self
            .targets
            .iter()
            .filter_map(|target| {
                target
                    .error
                    .as_ref()
                    .map(|error| format!("{}: {error}", target.input))
            })
            .collect::<Vec<_>>();
        (!failed.is_empty()).then(|| failed.join("; "))
    }
}

pub fn route_upper(route: Route) -> &'static str {
    match route {
        Route::Cable => "CABLE",
        Route::Wifi => "WIFI",
        Route::Lan => "LAN",
        Route::Tailscale => "TAILSCALE",
    }
}
