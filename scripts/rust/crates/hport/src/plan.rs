use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::forward::{Bind, Forward};
use crate::listener::Service;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Alias,
    Mirror,
}

impl From<Bind> for Kind {
    fn from(bind: Bind) -> Kind {
        match bind {
            Bind::Alias => Kind::Alias,
            Bind::Loopback4 | Bind::Loopback6 => Kind::Mirror,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Cancel(Forward),
    Alias(Forward),
    Mirror([Forward; 2]),
}

// All or nothing: localhost must not reach one server over IPv4 and another over IPv6
pub fn plan(
    services: &[Service],
    active: &BTreeSet<Forward>,
    may_try: impl Fn(Kind, u16) -> bool,
    mirror_free: impl Fn(u16) -> bool,
) -> Vec<Action> {
    let wanted = |forward: &Forward| {
        services
            .iter()
            .any(|service| service.port == forward.port && service.target == forward.target)
    };
    let mut actions = active
        .iter()
        .filter(|forward| !wanted(forward))
        .cloned()
        .map(Action::Cancel)
        .collect::<Vec<_>>();
    for service in services {
        let forward = |bind| Forward {
            bind,
            port: service.port,
            target: service.target,
        };
        let alias = forward(Bind::Alias);
        if !active.contains(&alias) && may_try(Kind::Alias, service.port) {
            actions.push(Action::Alias(alias));
        }
        let mirror = Bind::MIRROR.map(forward);
        let held = mirror
            .iter()
            .filter(|forward| active.contains(forward))
            .cloned()
            .collect::<Vec<_>>();
        match held.len() {
            2 => {}
            1 => actions.extend(held.into_iter().map(Action::Cancel)),
            _ if may_try(Kind::Mirror, service.port) && mirror_free(service.port) => {
                actions.push(Action::Mirror(mirror));
            }
            _ => {}
        }
    }
    actions
}

#[cfg(test)]
#[path = "../tests/unit/plan_tests.rs"]
mod tests;
