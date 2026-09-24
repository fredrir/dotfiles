use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Deserialize;

use crate::listener::Service;

// Below the Linux ephemeral range, skipping ports the kernel picked for a bind(0)
const MAX_PORT: u16 = 32767;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub max_port: u16,
    pub ignore_ports: BTreeSet<u16>,
    pub ignore_processes: BTreeSet<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_port: MAX_PORT,
            ignore_ports: BTreeSet::new(),
            ignore_processes: BTreeSet::new(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Config, String> {
        let path = path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                Config::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(error) => Err(format!("{}: {error}", path.display())),
        }
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        toml::from_str(text).map_err(|error| error.message().to_string())
    }

    pub fn admits(&self, service: &Service) -> bool {
        service.port <= self.max_port
            && !self.ignore_ports.contains(&service.port)
            && !self.ignore_processes.contains(&service.process)
    }
}

fn path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("HPORT_CONFIG").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|path| !path.is_empty()) {
        Some(base) => PathBuf::from(base),
        None => home()?.join(".config"),
    };
    Ok(base.join("hport/config.toml"))
}

pub fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_string())
}

#[cfg(test)]
#[path = "../tests/unit/config_tests.rs"]
mod tests;
