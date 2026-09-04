use std::process::Command;

use crate::host::Route;

// The ordered `Match exec` probes in ~/.ssh/config.d/05-* through 07-* run
// during config resolution, so asking ssh to resolve a host is the same
// decision the next connection to it will make, without opening one.
pub fn resolved(host: &str) -> Option<Route> {
    let output = Command::new("ssh").arg("-G").arg(host).output().ok()?;
    if !output.status.success() {
        return None;
    }
    classify(std::str::from_utf8(&output.stdout).ok()?)
}

pub fn classify(config: &str) -> Option<Route> {
    let mut hostname = None;
    let mut proxy = "";
    for line in config.lines() {
        let (key, value) = line.split_once(' ')?;
        match key {
            "hostname" => hostname = Some(value.trim()),
            "proxycommand" => proxy = value.trim(),
            _ => {}
        }
    }
    let hostname = hostname?;
    Some(match hostname {
        _ if hostname.starts_with("10.77.77.") => Route::Cable,
        _ if hostname.starts_with("10.77.78.") => Route::Wifi,
        _ if proxy.contains("home-lan-connect") => Route::Lan,
        _ => Route::Tailscale,
    })
}

#[cfg(test)]
#[path = "../tests/unit/ssh_tests.rs"]
mod tests;
