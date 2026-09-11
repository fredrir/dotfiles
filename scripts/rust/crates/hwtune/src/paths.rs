use std::env;
use std::path::PathBuf;

pub struct Paths {
    pub root: PathBuf,
    pub host: String,
}

impl Paths {
    pub fn discover(host: Option<&str>) -> Result<Self, String> {
        Ok(Self {
            root: repo_root(),
            host: host_name(host)?,
        })
    }

    pub fn bios_dir(&self) -> PathBuf {
        self.root.join("config").join("bios")
    }

    pub fn exports_dir(&self) -> PathBuf {
        self.bios_dir().join("exports")
    }

    pub fn spec_file(&self) -> PathBuf {
        self.bios_dir().join(format!("{}.dotfile", self.host))
    }

    pub fn stability_file(&self) -> PathBuf {
        self.bios_dir()
            .join(format!("{}-stability.dotfile", self.host))
    }
}

pub fn lact_config() -> PathBuf {
    env::var_os("HWTUNE_LACT_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/lact/config.yaml"))
}

pub fn repo_root() -> PathBuf {
    sysinfo::inventory::repo_root()
}

pub fn host_name(explicit: Option<&str>) -> Result<String, String> {
    let context = crate::bench::hosts::inventory_context();
    let requested = explicit
        .filter(|host| !host.is_empty())
        .map(str::to_owned)
        .or_else(|| env::var("HWTUNE_HOST").ok().filter(|host| !host.is_empty()));
    let resolved =
        sysinfo::inventory::resolve_with(&context, &[], requested.as_deref().unwrap_or(""), &[]);
    let host = if resolved.is_empty() {
        let names = sysinfo::inventory::local_hostnames();
        let known = crate::bench::hosts::load_hosts()?;
        let matched = sysinfo::inventory::match_hostname(&known, &names);
        if matched.is_empty() {
            names
                .first()
                .map(|name| name.split('.').next().unwrap_or(name).to_string())
                .unwrap_or_default()
        } else {
            matched
        }
    } else {
        resolved
    };
    if !sysinfo::inventory::valid_name(&host) {
        return Err("host must be a valid single path component".into());
    }
    Ok(host)
}

fn xdg(variable: &str, fallback: &[&str]) -> Result<PathBuf, String> {
    if let Some(dir) = env::var_os(variable) {
        return Ok(PathBuf::from(dir).join("hwtune"));
    }
    let home = env::var_os("HOME").ok_or("HOME is not set")?;
    let mut dir = PathBuf::from(home);
    for part in fallback {
        dir.push(part);
    }
    Ok(dir.join("hwtune"))
}

pub fn state_dir() -> Result<PathBuf, String> {
    xdg("XDG_STATE_HOME", &[".local", "state"])
}

pub fn cache_dir() -> Result<PathBuf, String> {
    xdg("XDG_CACHE_HOME", &[".cache"])
}

#[cfg(test)]
#[path = "../tests/unit/paths_tests.rs"]
mod tests;
