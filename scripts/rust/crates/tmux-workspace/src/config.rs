use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use nix::fcntl::{Flock, FlockArg};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

pub fn xdg(name: &str, fallback: &str) -> PathBuf {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(fallback))
}

pub fn expand(value: &str) -> PathBuf {
    let value = if value == "~" {
        home().to_string_lossy().into_owned()
    } else if let Some(rest) = value.strip_prefix("~/") {
        home().join(rest).to_string_lossy().into_owned()
    } else {
        value.to_owned()
    };
    let expression =
        regex::Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)")
            .expect("constant regex");
    PathBuf::from(
        expression
            .replace_all(&value, |caps: &regex::Captures<'_>| {
                let name = caps
                    .get(1)
                    .or_else(|| caps.get(2))
                    .expect("variable name")
                    .as_str();
                std::env::var(name).unwrap_or_else(|_| caps[0].to_owned())
            })
            .as_ref(),
    )
}

pub fn identity(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..12].to_owned()
}

pub fn clean(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect()
}

pub fn hostname() -> String {
    nix::unistd::gethostname()
        .map(|h| {
            h.to_string_lossy()
                .split('.')
                .next()
                .unwrap_or("host")
                .to_owned()
        })
        .unwrap_or_else(|_| "host".into())
}

pub fn private_dir(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    Ok(())
}

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or("state directory unavailable")?;
    private_dir(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

pub fn lock(path: &Path, wait: bool) -> Result<Flock<File>> {
    private_dir(path.parent().ok_or("lock directory unavailable")?)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    Flock::lock(
        file,
        if wait {
            FlockArg::LockExclusive
        } else {
            FlockArg::LockExclusiveNonblock
        },
    )
    .map_err(|(_, error)| format!("operation already running: {error}").into())
}

#[derive(Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
}

impl Paths {
    pub fn new(config: Option<PathBuf>) -> Self {
        Self {
            config: config
                .or_else(|| std::env::var_os("DOTFILES_TMUX_CONFIG").map(PathBuf::from))
                .unwrap_or_else(|| xdg("XDG_CONFIG_HOME", ".config").join("tmux")),
            data: std::env::var_os("DOTFILES_TMUX_PLUGIN_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| xdg("XDG_DATA_HOME", ".local/share").join("tmux/plugins")),
            state: xdg("XDG_STATE_HOME", ".local/state").join("tmux"),
        }
    }

    pub fn settings(&self) -> Result<Settings> {
        let path = self.config.join("workspace.toml");
        if !path.exists() {
            return Ok(Settings::default());
        }
        Ok(toml::from_str(&fs::read_to_string(path)?)?)
    }

    pub fn hosts(&self) -> Result<Vec<String>> {
        let explicit = std::env::var_os("DOTFILES_HOSTS_FILE").map(PathBuf::from);
        let linked = [&self.config, &self.config.join("plugins.lock.json")]
            .into_iter()
            .filter_map(|p| p.canonicalize().ok())
            .find_map(|p| {
                p.ancestors()
                    .map(|ancestor| ancestor.join("config/hosts.dotfile"))
                    .find(|p| p.is_file())
            });
        let path = explicit
            .or(linked.filter(|p| p.is_file()))
            .unwrap_or_else(|| home().join("dotfiles/config/hosts.dotfile"));
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let source = fs::read_to_string(path)?;
        let pattern = regex::Regex::new(r"(?m)^([A-Za-z0-9_][A-Za-z0-9_.-]*)\s*\{")?;
        Ok(pattern
            .captures_iter(&source)
            .map(|c| c[1].to_owned())
            .collect())
    }
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub projects: ProjectSettings,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectSettings {
    pub paths: Vec<String>,
    pub roots: Vec<String>,
    pub scan_children: bool,
    pub zoxide: bool,
    pub worktrees: bool,
    pub limit: usize,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            paths: Vec::new(),
            roots: Vec::new(),
            scan_children: true,
            zoxide: true,
            worktrees: true,
            limit: 300,
        }
    }
}
