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
    let fields: Vec<&str> = text.split(':').collect();
    if fields.len() != 5 || fields[0] != "v1" || fields[4] != "tls" {
        return Err("expected v1:<from>:<to>:<route>:tls".into());
    }
    let from = Host::from_name(fields[1])?;
    let to = Host::from_name(fields[2])?;
    if to != this || from != this.peer() {
        return Err(format!(
            "stamp says {} --> {}, but this process is on {}",
            from.name(),
            to.name(),
            this.name()
        ));
    }
    let route = match fields[3] {
        "cable" => Route::Cable,
        "wifi" => Route::Wifi,
        "tailscale" => Route::Tailscale,
        other => return Err(format!("unsupported TLS route: {other}")),
    };
    Ok(Session {
        from,
        to,
        route: Some(route),
        tls: true,
        client_address: None,
        client_port: None,
        server_address: Some(IpAddr::V4(to.address(route)?)),
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
    octets[0..3] == [192, 168, 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_addresses_identify_every_fixed_route() {
        let cases = [
            ("10.77.77.2", Route::Cable),
            ("10.77.78.2", Route::Wifi),
            ("100.126.231.24", Route::Tailscale),
            ("192.168.1.162", Route::Lan),
        ];
        for (server, route) in cases {
            let found = parse_ssh(&format!("192.0.2.1 54321 {server} 22"), Host::Archie).unwrap();
            assert_eq!(found.route, Some(route));
            assert_eq!(found.from, Host::Macie);
            assert_eq!(found.to, Host::Archie);
            assert!(!found.tls);
        }
    }

    #[test]
    fn an_unknown_ssh_address_stays_unknown() {
        let found = parse_ssh("203.0.113.5 1 172.16.0.2 22", Host::Archie).unwrap();
        assert_eq!(found.route, None);
    }

    #[test]
    fn tls_stamps_are_strict_and_host_bound() {
        for (this, peer) in [(Host::Archie, Host::Macie), (Host::Macie, Host::Archie)] {
            let stamp = format!("v1:{}:{}:cable:tls", peer.name(), this.name());
            let found = parse_tls(&stamp, this).unwrap();
            assert_eq!(found.route, Some(Route::Cable));
            assert!(found.tls);
            assert_eq!(
                found.domain.as_deref(),
                Some(format!("{}-cable", this.name()).as_str())
            );
        }
        assert!(parse_tls("v1:archie:macie:cable:tls", Host::Archie).is_err());
        assert!(parse_tls("v1:macie:archie:lan:tls", Host::Archie).is_err());
        assert!(parse_tls("anything", Host::Archie).is_err());
    }

    #[test]
    fn malformed_ssh_state_is_rejected() {
        assert!(parse_ssh("", Host::Macie).is_err());
        assert!(parse_ssh("bad 1 10.77.77.1 22", Host::Macie).is_err());
        assert!(parse_ssh("1.2.3.4 1 10.77.77.1 22 extra", Host::Macie).is_err());
    }
}
