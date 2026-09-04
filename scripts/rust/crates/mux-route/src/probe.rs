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
#[path = "../tests/unit/probe_tests.rs"]
mod tests;
