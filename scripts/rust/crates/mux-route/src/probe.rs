//! Asking every route at once whether the peer's mux is on the other end.

use std::net::SocketAddrV4;

use hostkit::{Host, MUX_PORT, Route};

use crate::domain::{self, ROUTES};

/// What one route had to say, whether or not it said yes.
pub struct Answer {
    pub route: Route,
    pub peer: SocketAddrV4,
    pub up: bool,
}

/// Probe every mux route in parallel, because a dead one costs the whole
/// timeout and there is no reason to spend it three times over.
pub fn probe(this: Host, peer: Host) -> Result<Vec<Answer>, String> {
    let mut targets = Vec::with_capacity(ROUTES.len());
    for route in ROUTES {
        targets.push((route, SocketAddrV4::new(peer.address(route)?, MUX_PORT)));
    }
    Ok(std::thread::scope(|scope| {
        let asked: Vec<_> = targets
            .into_iter()
            .map(|(route, address)| {
                let probe = scope.spawn(move || route.answers(this, peer, MUX_PORT));
                (route, address, probe)
            })
            .collect();
        asked
            .into_iter()
            .map(|(route, address, probe)| Answer {
                route,
                peer: address,
                // A probe that panicked did not answer, which is down.
                up: probe.join().unwrap_or(false),
            })
            .collect()
    }))
}

/// The domain for the first route that answered, or why there is none to name.
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

    fn answers(up: [bool; 3]) -> Vec<Answer> {
        ROUTES
            .into_iter()
            .zip(up)
            .map(|(route, up)| Answer {
                route,
                peer: SocketAddrV4::new(Ipv4Addr::LOCALHOST, MUX_PORT),
                up,
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
