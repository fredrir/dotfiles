use std::collections::HashMap;
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use crate::protocol::{self, Response};

pub const GRANT: Duration = Duration::from_secs(30 * 60);

pub struct Broker {
    vaults: Vec<String>,
    grant: Duration,
    granted: HashMap<String, Grant>,
}

struct Grant {
    until: Instant,
    value: Zeroizing<String>,
}

impl Broker {
    pub fn new(vaults: Vec<String>, grant: Duration) -> Broker {
        Broker {
            vaults,
            grant,
            granted: HashMap::new(),
        }
    }

    pub fn granted(&mut self, reference: &str, now: Instant) -> bool {
        self.granted.retain(|_, grant| grant.until > now);
        self.granted.contains_key(reference)
    }

    pub fn resolve(
        &mut self,
        reference: &str,
        peer: &str,
        now: Instant,
        approve: impl FnOnce(&str) -> Result<(), String>,
        fetch: impl FnOnce(&str) -> Result<Zeroizing<String>, String>,
    ) -> Response {
        if let Err(reason) = protocol::check(reference, &self.vaults) {
            return Response::Refused(reason);
        }
        if self.granted(reference, now) {
            return Response::Value(self.granted[reference].value.clone());
        }
        if let Err(reason) = approve(&prompt(peer, reference)) {
            return Response::Denied(reason);
        }
        match fetch(reference) {
            Ok(value) => {
                let grant = Grant {
                    until: now + self.grant,
                    value: value.clone(),
                };
                self.granted.insert(reference.to_string(), grant);
                Response::Value(value)
            }
            Err(reason) => Response::Refused(reason),
        }
    }
}

// macOS shows this as "op-bridge is trying to <prompt>."
pub fn prompt(peer: &str, reference: &str) -> String {
    format!("send {reference} to {peer}")
}

#[cfg(test)]
#[path = "../tests/unit/broker_tests.rs"]
mod tests;
