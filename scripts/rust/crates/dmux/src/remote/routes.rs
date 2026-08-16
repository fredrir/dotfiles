//! Route policy (plan §12.3, §8.3/§8.4): eligibility filtering, priority
//! order, the typed route-outcome token vocabulary, and the enrollment
//! network-class heuristic. Route ROWS live in the registry; this module is
//! the policy layer above them.

use crate::registry::{NetworkClass, RouteRow};

/// Stable typed outcome tokens recorded via
/// `Registry::record_route_outcome` on EVERY attempt (ADR 009 §4).
pub mod outcome {
    /// Envelope exchanged and validated end to end.
    pub const OK: &str = "ok";
    /// Enumerated pre-authentication transport failure (connection
    /// refused/reset, no route, DNS, connect-stage timeout, spawn failure).
    /// The only class that may try the next verified route.
    pub const TRANSPORT_UNREACHABLE: &str = "transport_unreachable";
    /// SSH authentication failure (never retried).
    pub const AUTH_FAILED: &str = "auth_failed";
    /// Host-key verification/identity failure (never retried).
    pub const HOST_KEY_FAILED: &str = "host_key_failed";
    /// The remote command is missing/incompatible (never retried).
    pub const COMMAND_MISSING: &str = "command_missing";
    /// The dmux-imposed deadline elapsed after the connection was
    /// established (never retried; a connect-stage timeout is ssh's own
    /// "Connection timed out" and classifies as transport).
    pub const TIMEOUT: &str = "timeout";
    /// Unparseable/ill-formed response document (never retried).
    pub const MALFORMED_RESPONSE: &str = "malformed_response";
    /// Exact protocol version mismatch (never retried).
    pub const PROTOCOL_MISMATCH: &str = "protocol_mismatch";
    /// Responder is not the enrolled HostUid (never retried).
    pub const HOST_IDENTITY_CHANGED: &str = "host_identity_changed";
    /// RegistryUid/lineage conflict per §12.1 (never retried).
    pub const LINEAGE_CONFLICT: &str = "lineage_conflict";
    /// The agent answered with a typed error envelope (never retried).
    pub const AGENT_ERROR: &str = "agent_error";
}

/// Eligible routes for an operation: enabled, and when the operation needs
/// a capability, only routes requiring none or exactly that capability.
/// Input order (priority ASC, route_id ASC from `routes_for`) is preserved.
pub fn eligible(routes: Vec<RouteRow>, need_capability: Option<&str>) -> Vec<RouteRow> {
    routes
        .into_iter()
        .filter(|r| r.enabled)
        .filter(|r| match (&r.required_capability, need_capability) {
            (None, _) => true,
            (Some(required), Some(need)) => required == need,
            (Some(_), None) => true,
        })
        .collect()
}

/// Default priority for a network class (plan §8.4 order: USB, Tailscale,
/// then other enrolled routes; lower tries first).
pub fn default_priority(class: NetworkClass) -> i64 {
    match class {
        NetworkClass::Usb => 10,
        NetworkClass::Tailscale => 20,
        NetworkClass::Lan => 30,
        NetworkClass::Other => 40,
    }
}

/// Enrollment network-class heuristic (ADR 009 §4). Today's facts: the USB
/// link is the `archie` ssh alias / 10.77.77.0/24; Tailscale peers are
/// MagicDNS names (`*.ts.net`) or CGNAT 100.64.0.0/10 addresses. Everything
/// else is `Other` — never guessed further.
pub fn classify_endpoint(endpoint: &str) -> NetworkClass {
    let host = endpoint.rsplit('@').next().unwrap_or(endpoint);
    let host = host.strip_suffix('.').unwrap_or(host);
    if host == "archie" || host.starts_with("10.77.77.") {
        return NetworkClass::Usb;
    }
    if host.ends_with(".ts.net") || host.contains(".ts.net:") {
        return NetworkClass::Tailscale;
    }
    if let Some(class) = classify_ipv4(host) {
        return class;
    }
    NetworkClass::Other
}

fn classify_ipv4(host: &str) -> Option<NetworkClass> {
    let octets: Vec<u8> = host
        .split('.')
        .map(|part| part.parse::<u8>().ok())
        .collect::<Option<Vec<u8>>>()?;
    if octets.len() != 4 {
        return None;
    }
    // Tailscale CGNAT range 100.64.0.0/10.
    if octets[0] == 100 && (64..=127).contains(&octets[1]) {
        return Some(NetworkClass::Tailscale);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::HostUid;
    use crate::registry::Transport;
    use uuid::Uuid;

    fn route(id: i64, enabled: bool, capability: Option<&str>) -> RouteRow {
        RouteRow {
            route_id: id,
            host_uid: HostUid(Uuid::nil()),
            transport: Transport::Openssh,
            endpoint: format!("endpoint-{id}"),
            username: None,
            wez_domain: None,
            network_class: NetworkClass::Other,
            priority: id,
            required_capability: capability.map(str::to_string),
            trust_fingerprint: None,
            enabled,
            last_outcome: None,
            last_outcome_at: None,
        }
    }

    #[test]
    fn eligibility_filters_disabled_and_mismatched_capability() {
        let routes = vec![
            route(1, true, None),
            route(2, false, None),
            route(3, true, Some("wez")),
        ];
        let plain: Vec<i64> = eligible(routes.clone(), None)
            .iter()
            .map(|r| r.route_id)
            .collect();
        assert_eq!(plain, vec![1, 3]);
        let wez: Vec<i64> = eligible(routes.clone(), Some("wez"))
            .iter()
            .map(|r| r.route_id)
            .collect();
        assert_eq!(wez, vec![1, 3]);
        let tmux: Vec<i64> = eligible(routes, Some("tmux"))
            .iter()
            .map(|r| r.route_id)
            .collect();
        assert_eq!(tmux, vec![1]);
    }

    #[test]
    fn endpoint_classification_matches_todays_facts() {
        assert_eq!(classify_endpoint("archie"), NetworkClass::Usb);
        assert_eq!(classify_endpoint("fredrir@archie"), NetworkClass::Usb);
        assert_eq!(classify_endpoint("10.77.77.2"), NetworkClass::Usb);
        assert_eq!(
            classify_endpoint("archie.tail1234.ts.net"),
            NetworkClass::Tailscale
        );
        assert_eq!(classify_endpoint("100.101.5.9"), NetworkClass::Tailscale);
        assert_eq!(classify_endpoint("100.63.0.1"), NetworkClass::Other);
        assert_eq!(classify_endpoint("100.128.0.1"), NetworkClass::Other);
        assert_eq!(classify_endpoint("archie-ts"), NetworkClass::Other);
        assert_eq!(classify_endpoint("example.com"), NetworkClass::Other);
        assert_eq!(classify_endpoint("192.168.1.4"), NetworkClass::Other);
    }

    #[test]
    fn priority_order_is_usb_tailscale_lan_other() {
        let mut classes = [
            NetworkClass::Other,
            NetworkClass::Usb,
            NetworkClass::Lan,
            NetworkClass::Tailscale,
        ];
        classes.sort_by_key(|c| default_priority(*c));
        assert_eq!(
            classes,
            [
                NetworkClass::Usb,
                NetworkClass::Tailscale,
                NetworkClass::Lan,
                NetworkClass::Other
            ]
        );
    }
}
