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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub name: String,
    pub source: PathBuf,
    #[serde(default)]
    pub env: Vec<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fold {
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    5000
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let config: Config = toml::from_str(text).map_err(|error| error.to_string())?;
        for command in &config.fold.commands {
            if command.split_whitespace().next().is_none() {
                return Err("fold.commands: empty command".to_string());
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
