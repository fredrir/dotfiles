use std::process::Command;
use std::time::Duration;

use hostkit::process::{self, CaptureLimits};

pub const UNITS: [&str; 3] = ["fan2go", "lactd", "nvidia-persistenced"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceState {
    pub unit: String,
    pub active: String,
    pub enabled: String,
}

fn query(verb: &str, unit: &str) -> String {
    let mut command = Command::new("systemctl");
    command.args([verb, unit]);
    match process::output(
        &mut command,
        CaptureLimits::default(),
        Duration::from_secs(5),
    ) {
        Ok(captured) => {
            let text = String::from_utf8_lossy(&captured.stdout).trim().to_string();
            if text.is_empty() {
                "unknown".into()
            } else {
                text
            }
        }
        Err(_) => "unknown".into(),
    }
}

pub fn state(unit: &str) -> ServiceState {
    ServiceState {
        unit: unit.to_string(),
        active: query("is-active", unit),
        enabled: query("is-enabled", unit),
    }
}

pub fn all() -> Vec<ServiceState> {
    UNITS.iter().map(|unit| state(unit)).collect()
}
