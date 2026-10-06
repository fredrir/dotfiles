use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use crate::protocol::{self, Response};

pub const GRANT: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    // Served from memory until macie sleeps
    Silent,
    // 1Password's prompt per reference, then a grant
    Prompt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Peer,
    Local,
}

impl Origin {
    pub fn name(self) -> &'static str {
        match self {
            Origin::Peer => "archie",
            Origin::Local => "macie",
        }
    }
}

pub struct Policy {
    silent: Vec<String>,
    prompt: Vec<String>,
}

impl Policy {
    pub fn new(silent: Vec<String>, prompt: Vec<String>) -> Result<Policy, String> {
        if let Some(vault) = silent
            .iter()
            .find(|vault| prompt.iter().any(|other| other.eq_ignore_ascii_case(vault)))
        {
            return Err(format!("vault {vault} is both silent and prompted"));
        }
        Ok(Policy { silent, prompt })
    }

    pub fn tier(&self, reference: &str) -> Result<Tier, String> {
        let vault = protocol::validate(reference)?;
        let listed = |vaults: &[String]| vaults.iter().any(|v| v.eq_ignore_ascii_case(vault));
        if listed(&self.silent) {
            Ok(Tier::Silent)
        } else if listed(&self.prompt) {
            Ok(Tier::Prompt)
        } else {
            Err(format!("vault {vault} is not shared"))
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "silent: {}; Touch ID: {}",
            list(&self.silent),
            list(&self.prompt)
        )
    }
}

fn list(vaults: &[String]) -> String {
    if vaults.is_empty() {
        "none".to_string()
    } else {
        vaults.join(", ")
    }
}

pub struct Broker {
    policy: Policy,
    grant: Duration,
    cached: HashMap<String, Cached>,
    known: BTreeSet<String>,
    known_changed: bool,
    stale: bool,
}

struct Cached {
    until: Option<Instant>,
    value: Zeroizing<String>,
}

impl Broker {
    pub fn new(policy: Policy, grant: Duration, known: BTreeSet<String>) -> Broker {
        Broker {
            policy,
            grant,
            cached: HashMap::new(),
            known,
            known_changed: false,
            stale: true,
        }
    }

    pub fn cached(&mut self, reference: &str, now: Instant) -> bool {
        self.cached
            .retain(|_, entry| entry.until.is_none_or(|until| until > now));
        self.cached.contains_key(reference)
    }

    pub fn resolve(
        &mut self,
        reference: &str,
        now: Instant,
        sign_out: impl FnOnce() -> Result<(), String>,
        fetch: impl FnOnce(&str) -> Result<Zeroizing<String>, String>,
    ) -> Response {
        let tier = match self.policy.tier(reference) {
            Ok(tier) => tier,
            Err(reason) => return Response::Refused(reason),
        };

        if self.cached(reference, now) {
            return Response::Value(self.cached[reference].value.clone());
        }
        if tier == Tier::Prompt
            && let Err(reason) = sign_out()
        {
            return Response::Denied(reason);
        }
        match fetch(reference) {
            Ok(value) => {
                let until = match tier {
                    Tier::Silent => None,
                    Tier::Prompt => Some(now + self.grant),
                };
                if tier == Tier::Silent {
                    self.known_changed |= self.known.insert(reference.to_string());
                }
                self.cached.insert(
                    reference.to_string(),
                    Cached {
                        until,
                        value: value.clone(),
                    },
                );
                Response::Value(value)
            }
            Err(reason) if tier == Tier::Prompt => Response::Denied(reason),
            Err(reason) => Response::Refused(reason),
        }
    }

    pub fn forget(&mut self) {
        self.cached.clear();
        self.stale = true;
    }

    // True once after startup or a sleep, so the first fetch refills the rest
    pub fn take_stale(&mut self) -> bool {
        std::mem::take(&mut self.stale)
    }

    // Silent references used before but not in memory, e.g. after a sleep
    pub fn missing(&self) -> Vec<String> {
        self.known
            .iter()
            .filter(|reference| !self.cached.contains_key(*reference))
            .filter(|reference| self.policy.tier(reference) == Ok(Tier::Silent))
            .cloned()
            .collect()
    }

    pub fn store(&mut self, reference: &str, value: Zeroizing<String>) {
        if self.policy.tier(reference) == Ok(Tier::Silent) {
            let cached = Cached { until: None, value };
            self.cached.insert(reference.to_string(), cached);
        }
    }

    pub fn take_known(&mut self) -> Option<Vec<String>> {
        std::mem::take(&mut self.known_changed).then(|| self.known.iter().cloned().collect())
    }
}

#[cfg(test)]
#[path = "../tests/unit/broker_tests.rs"]
mod tests;
