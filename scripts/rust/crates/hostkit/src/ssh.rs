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
mod tests {
    use super::*;

    #[test]
    fn a_cabled_address_is_the_cable() {
        assert_eq!(
            classify("hostname 10.77.77.2\nuser fredrir\n"),
            Some(Route::Cable)
        );
    }

    #[test]
    fn a_direct_wifi_address_is_not_mistaken_for_the_cable() {
        assert_eq!(classify("hostname 10.77.78.2\n"), Some(Route::Wifi));
    }

    #[test]
    fn the_filtered_lan_is_named_by_its_proxy_rather_than_its_address() {
        let config =
            "hostname archie.local\nproxycommand /home/f/.ssh/bin/home-lan-connect %h %p\n";
        assert_eq!(classify(config), Some(Route::Lan));
    }

    #[test]
    fn anything_else_resolved_is_tailscale() {
        assert_eq!(classify("hostname archie\n"), Some(Route::Tailscale));
        assert_eq!(
            classify("hostname 100.126.231.24\n"),
            Some(Route::Tailscale)
        );
    }

    #[test]
    fn a_config_with_no_hostname_resolves_to_no_route() {
        assert_eq!(classify(""), None);
        assert_eq!(classify("user fredrir\nport 22\n"), None);
    }
}
