use hostkit::{Host, MUX_PORT, RouteProbe};

use crate::domain::{self, ROUTES};

pub type Answer = RouteProbe;

pub fn probe(this: Host, peer: Host) -> Result<Vec<Answer>, String> {
    Ok(hostkit::probe_routes(this, peer, MUX_PORT, &ROUTES).routes)
}

pub fn pick(peer: Host, answers: &[Answer]) -> Result<String, String> {
    answers
        .iter()
        .find(|answer| answer.up)
        .map(|answer| domain::name(peer, answer.route))
        .ok_or_else(|| format!("no route to {} answered on port {MUX_PORT}", peer.name()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    fn answers(up: [bool; 3]) -> Vec<Answer> {
        ROUTES
            .into_iter()
            .zip(up)
            .map(|(route, up)| Answer {
                route,
                local_address: Some(Ipv4Addr::LOCALHOST),
                peer_address: Some(Ipv4Addr::LOCALHOST),
                port: MUX_PORT,
                up,
                elapsed: Duration::ZERO,
                error: (!up).then(|| "refused".into()),
            })
            .collect()
    }

    #[test]
    fn the_mux_is_probed_on_its_own_port_rather_than_sshd() {
        assert_eq!(MUX_PORT, 8443);
    }

    #[test]
    fn the_first_route_that_answered_wins() {
        assert_eq!(
            pick(Host::Archie, &answers([true, true, true])).unwrap(),
            "archie-cable"
        );
        assert_eq!(
            pick(Host::Archie, &answers([false, true, true])).unwrap(),
            "archie-wifi"
        );
        assert_eq!(
            pick(Host::Archie, &answers([false, false, true])).unwrap(),
            "archie-tailscale"
        );
    }

    #[test]
    fn nothing_answering_is_a_failure_that_names_the_port() {
        let reason = pick(Host::Archie, &answers([false, false, false])).unwrap_err();
        assert!(reason.contains("archie"), "{reason}");
        assert!(reason.contains("8443"), "{reason}");
        assert!(pick(Host::Macie, &[]).is_err());
    }
}
