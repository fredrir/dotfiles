use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const RELATIVE_PATH: &str = "config/zsh/build.toml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub output: PathBuf,
    #[serde(default)]
    pub ambient: Vec<String>,
    #[serde(default, rename = "target")]
    pub targets: Vec<Target>,
    #[serde(default)]
    pub fold: Fold,
    #[serde(default)]
    pub system: System,
    #[serde(default)]
    pub defer: Defer,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub name: String,
    pub source: PathBuf,
    #[serde(default)]
    pub env: Vec<PathBuf>,
    /// Linked as `~/.zprofile`; compiled in with the global startup files.
    pub profile: Option<PathBuf>,
    /// Compiles the global startup files zsh reads before `.zshrc` into this target.
    #[serde(default)]
    pub system: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct System {
    #[serde(default = "default_global_dir")]
    pub dir: PathBuf,
    /// Variables known while the global startup files run.
    #[serde(default)]
    pub ambient: Vec<String>,
    /// Prefix for `path_helper`'s `/etc/paths` and `/etc/manpaths` data, as `PATH_HELPER_ROOT`.
    #[serde(default)]
    pub path_helper_root: String,
}

impl Default for System {
    fn default() -> Self {
        Self {
            dir: default_global_dir(),
            ambient: Vec::new(),
            path_helper_root: String::new(),
        }
    }
}

/// Where zsh was built to read `zprofile` and `zshrc`.
fn default_global_dir() -> PathBuf {
    PathBuf::from(if cfg!(target_os = "macos") {
        "/etc"
    } else {
        "/etc/zsh"
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fold {
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

impl Default for Fold {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            timeout_ms: default_timeout(),
        }
    }
}

fn default_timeout() -> u64 {
    5000
}

/// Code run after the first prompt, through the dotfiles' `defer`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defer {
    /// oh-my-zsh's compinit section.
    #[serde(default)]
    pub compinit: bool,
    /// oh-my-zsh plugin names; glob patterns.
    #[serde(default)]
    pub plugins: Vec<String>,
    /// `cached_eval` names; glob patterns.
    #[serde(default)]
    pub evals: Vec<String>,
    /// Sourced files relative to the root; glob patterns.
    #[serde(default)]
    pub files: Vec<String>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let config: Config = toml::from_str(text).map_err(|error| error.to_string())?;
        if let Some(target) = config
            .targets
            .iter()
            .find(|target| target.profile.is_some() && !target.system)
        {
            return Err(format!(
                "target {}: profile needs system = true",
                target.name
            ));
        }
        if config.targets.iter().filter(|target| target.system).count() > 1 {
            return Err("system = true: more than one target".to_string());
        }
        for command in &config.fold.commands {
            if command.split_whitespace().next().is_none() {
                return Err("fold.commands: empty command".to_string());
            }
        }
        let defer = &config.defer;
        for (key, patterns) in [
            ("plugins", &defer.plugins),
            ("evals", &defer.evals),
            ("files", &defer.files),
        ] {
            for pattern in patterns {
                glob::Pattern::new(pattern)
                    .map_err(|error| format!("defer.{key}: {pattern}: {error}"))?;
            }
        }
        Ok(config)
    }

    pub fn fold_commands(&self) -> Vec<Vec<String>> {
        self.fold
            .commands
            .iter()
            .map(|command| command.split_whitespace().map(str::to_string).collect())
            .collect()
    }
}

#[cfg(test)]
#[path = "../tests/unit/config_tests.rs"]
mod tests;
