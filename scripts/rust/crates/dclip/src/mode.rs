use std::ffi::OsString;

use hostkit::Host;
use hostkit::session::{self, Stamp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Mux(Stamp),
    Ssh,
    Native,
    Terminal,
}

pub fn detect(this: Host, native: bool, env: impl Fn(&str) -> Option<OsString>) -> Mode {
    let set = |name| env(name).filter(|value| !value.is_empty());
    let stamp =
        set("HWIRE_SESSION").and_then(|stamp| session::parse(&stamp.to_string_lossy(), this).ok());
    if let Some(stamp) = stamp {
        Mode::Mux(stamp)
    } else if set("SSH_CONNECTION").is_some() || set("SSH_TTY").is_some() {
        Mode::Ssh
    } else if native {
        Mode::Native
    } else {
        Mode::Terminal
    }
}

#[cfg(test)]
#[path = "../tests/unit/mode_tests.rs"]
mod tests;
