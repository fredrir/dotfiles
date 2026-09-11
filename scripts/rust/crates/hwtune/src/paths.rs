use std::env;
use std::path::{Path, PathBuf};

pub struct Paths {
    pub root: PathBuf,
    pub host: String,
}

impl Paths {
    pub fn discover(host: Option<&str>) -> Result<Self, String> {
        Ok(Self {
            root: repo_root()?,
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

    pub fn benchmarks_dir(&self) -> PathBuf {
        env::var_os("SYSINFO_BENCHMARKS")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.root.join("benchmarks"))
    }
}

pub fn lact_config() -> PathBuf {
    env::var_os("HWTUNE_LACT_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/lact/config.yaml"))
}

pub fn repo_root() -> Result<PathBuf, String> {
    if let Some(root) = env::var_os("DOTFILE_ROOT") {
        return Ok(PathBuf::from(root));
    }
    let start = env::current_dir().map_err(|e| format!("current directory: {e}"))?;
    let mut dir: &Path = &start;
    loop {
        if dir.join("config").join("targets.dotfile").is_file() {
            return Ok(dir.to_path_buf());
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return Err("repository root not found; set DOTFILE_ROOT".into()),
        }
    }
}

pub fn host_name(explicit: Option<&str>) -> Result<String, String> {
    if let Some(host) = explicit {
        return Ok(host.to_string());
    }
    if let Ok(host) = env::var("HWTUNE_HOST")
        && !host.is_empty()
    {
        return Ok(host);
    }
    let raw = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map_err(|e| format!("hostname: {e}"))?;
    let short = raw.trim().split('.').next().unwrap_or_default().to_string();
    if short.is_empty() {
        return Err("hostname is empty".into());
    }
    Ok(short)
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
