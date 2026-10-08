use hostkit::{Host, Route};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub origin: Host,
    pub route: Route,
}

pub fn parse(text: &str, this: Host) -> Result<Stamp, String> {
    let fields: Vec<&str> = text.split(':').collect();
    let [version, origin, destination, route, transport] = fields[..] else {
        return Err("bad HWIRE_SESSION".into());
    };
    if version != "v1" || transport != "tls" {
        return Err("bad HWIRE_SESSION".into());
    }
    let origin = Host::from_name(origin)?;
    let destination = Host::from_name(destination)?;
    if destination != this || origin != this.peer() {
        return Err(format!(
            "HWIRE_SESSION is {} --> {}, not {} --> {}",
            origin.name(),
            destination.name(),
            this.peer().name(),
            this.name()
        ));
    }
    let route = route_named(route).ok_or_else(|| format!("unknown route: {route}"))?;
    Ok(Stamp { origin, route })
}

pub fn route_named(name: &str) -> Option<Route> {
    Route::every()
        .into_iter()
        .find(|route| route.name() == name)
}

#[cfg(test)]
#[path = "../tests/unit/session_tests.rs"]
mod tests;
