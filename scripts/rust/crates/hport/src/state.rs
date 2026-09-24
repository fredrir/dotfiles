use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::home;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub pid: u32,
    pub peer: String,
    pub route: Option<String>,
    pub connected: bool,
    pub error: Option<String>,
    pub services: Vec<Entry>,
}

impl State {
    pub fn idle(peer: hostkit::Host) -> State {
        State {
            pid: std::process::id(),
            peer: peer.name().to_string(),
            route: None,
            connected: false,
            error: None,
            services: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub port: u16,
    pub process: String,
    pub alias: Status,
    pub mirror: Status,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "lowercase")]
pub enum Status {
    Active,
    Pending,
    Busy(Option<String>),
    Failed(String),
}

pub struct Paths {
    pub state: PathBuf,
    pub socket: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Paths, String> {
        let base = match std::env::var_os("XDG_STATE_HOME").filter(|path| !path.is_empty()) {
            Some(base) => PathBuf::from(base),
            None => home()?.join(".local/state"),
        };
        let directory = base.join("hport");
        Ok(Paths {
            state: directory.join("state.json"),
            socket: directory.join("master.sock"),
        })
    }

    pub fn prepare(&self) -> Result<(), String> {
        let Some(directory) = self.state.parent() else {
            return Ok(());
        };
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("{}: {error}", directory.display()))
    }
}

pub fn write(paths: &Paths, state: &State) -> Result<(), String> {
    let text = serde_json::to_string_pretty(state).map_err(|error| error.to_string())?;
    let staging = paths.state.with_extension("json.tmp");
    std::fs::write(&staging, text + "\n")
        .and_then(|()| std::fs::rename(&staging, &paths.state))
        .map_err(|error| format!("{}: {error}", paths.state.display()))
}

pub fn read(paths: &Paths) -> Result<Option<State>, String> {
    match std::fs::read_to_string(&paths.state) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| format!("{}: {error}", paths.state.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", paths.state.display())),
    }
}

#[cfg(test)]
#[path = "../tests/unit/state_tests.rs"]
mod tests;
