use clap::ValueEnum;

use crate::{Host, Route};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub origin: Host,
    pub destination: Host,
    pub route: Route,
}

pub fn parse(text: &str, this: Host) -> Result<Stamp, String> {
    let fields: Vec<&str> = text.split(':').collect();
    let ["v1", origin, destination, route, "tls"] = fields[..] else {
        return Err("expected v1:<from>:<to>:<route>:tls".into());
    };
    let origin = Host::from_name(origin)?;
    let destination = Host::from_name(destination)?;
    if destination != this || origin != this.peer() {
        return Err(format!(
            "stamp says {} --> {}, but this process is on {}",
            origin.name(),
            destination.name(),
            this.name()
        ));
    }
    let route =
        Route::from_str(route, false).map_err(|_| format!("unsupported TLS route: {route}"))?;
    Ok(Stamp {
        origin,
        destination,
        route,
    })
}

#[cfg(test)]
#[path = "../tests/unit/session_tests.rs"]
mod tests;
