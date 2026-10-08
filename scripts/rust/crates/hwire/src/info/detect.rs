use std::net::{IpAddr, Ipv4Addr};

use hostkit::{Host, Route};

use super::model::Session;

pub struct Detection {
    pub session: Option<Session>,
    pub warnings: Vec<String>,
}

pub fn session(this: Host) -> Detection {
    let mut warnings = Vec::new();
    if let Some(value) = std::env::var_os("SSH_CONNECTION") {
        let text = value.to_string_lossy();
        match parse_ssh(&text, this) {
            Ok(session) => {
                return Detection {
                    session: Some(session),
                    warnings,
                };
            }
            Err(error) => warnings.push(format!("SSH_CONNECTION: {error}")),
        }
    }

    if let Some(value) = std::env::var_os("HWIRE_SESSION") {
        let text = value.to_string_lossy();
        match parse_tls(&text, this) {
            Ok(session) => {
                return Detection {
                    session: Some(session),
                    warnings,
                };
            }
            Err(error) => warnings.push(format!("HWIRE_SESSION: {error}")),
        }
    }

    Detection {
        session: None,
        warnings,
    }
}

pub fn parse_ssh(text: &str, this: Host) -> Result<Session, String> {
    let mut fields = text.split_whitespace();
    let client_address: IpAddr = field(&mut fields, "client address")?
        .parse()
        .map_err(|_| "invalid client address".to_string())?;
    let client_port = field(&mut fields, "client port")?
        .parse()
        .map_err(|_| "invalid client port".to_string())?;
    let server_address: IpAddr = field(&mut fields, "server address")?
        .parse()
        .map_err(|_| "invalid server address".to_string())?;
    let server_port = field(&mut fields, "server port")?
        .parse()
        .map_err(|_| "invalid server port".to_string())?;
    if fields.next().is_some() {
        return Err("expected exactly four fields".into());
    }
    Ok(Session {
        from: this.peer(),
        to: this,
        route: route_for(server_address, this),
        tls: false,
        client_address: Some(client_address),
        client_port: Some(client_port),
        server_address: Some(server_address),
        server_port: Some(server_port),
        domain: None,
        evidence: "SSH_CONNECTION",
    })
}

pub fn parse_tls(text: &str, this: Host) -> Result<Session, String> {
    let stamp = hostkit::session::parse(text, this)?;
    let (from, to, route) = (stamp.origin, stamp.destination, stamp.route);
    // The LAN pair is resolved live, so a LAN pane's server address stays unknown.
    let server_address = match route {
        Route::Lan => None,
        route => Some(IpAddr::V4(to.address(route)?)),
    };
    Ok(Session {
        from,
        to,
        route: Some(route),
        tls: true,
        client_address: None,
        client_port: None,
        server_address,
        server_port: Some(hostkit::MUX_PORT),
        domain: Some(format!("{}-{}", to.name(), route.name())),
        evidence: "HWIRE_SESSION",
    })
}

fn field<'a>(fields: &mut impl Iterator<Item = &'a str>, name: &str) -> Result<&'a str, String> {
    fields.next().ok_or_else(|| format!("missing {name}"))
}

fn route_for(address: IpAddr, this: Host) -> Option<Route> {
    let IpAddr::V4(address) = address else {
        return None;
    };
    for route in [Route::Cable, Route::Wifi, Route::Tailscale] {
        if this.address(route).ok() == Some(address) {
            return Some(route);
        }
    }
    is_home_lan(address).then_some(Route::Lan)
}

fn is_home_lan(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0..3] == [192, 168, 50]
}

#[cfg(test)]
#[path = "../../tests/unit/info/detect_tests.rs"]
mod tests;
