use hostkit::{Host, Route};

pub const ROUTES: [Route; 3] = [Route::Cable, Route::Wifi, Route::Tailscale];

pub fn name(peer: Host, route: Route) -> String {
    format!("{}-{}", peer.name(), route.name())
}

pub fn target(named: Option<Host>, this: Host) -> Result<Host, String> {
    let Some(host) = named else {
        return Ok(this.peer());
    };
    if host == this {
        return Err(format!(
            "{} is this machine; its panes are already in localmux",
            host.name()
        ));
    }
    Ok(host)
}

#[cfg(test)]
#[path = "../tests/unit/domain_tests.rs"]
mod tests;
