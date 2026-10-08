use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use hostkit::Route;

use crate::session::route_named;

pub fn path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(state.join("dclip/route"))
}

pub fn load(path: &Path) -> Option<Route> {
    route_named(fs::read_to_string(path).ok()?.trim())
}

pub fn save(path: &Path, route: Route) -> io::Result<()> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    let staged = path.with_extension(format!("{}", std::process::id()));
    fs::write(&staged, format!("{}\n", route.name()))?;
    fs::rename(&staged, path).inspect_err(|_| {
        let _ = fs::remove_file(&staged);
    })
}

#[cfg(test)]
#[path = "../tests/unit/state_tests.rs"]
mod tests;
