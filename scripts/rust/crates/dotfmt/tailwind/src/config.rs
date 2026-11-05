use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    Off,
    On,
    #[default]
    Auto,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OrderSource {
    #[default]
    Auto,
    Project,
    Rust,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub mode: Mode,
    pub order: OrderSource,
    pub functions: Vec<String>,
    pub attributes: Vec<String>,
    pub dedupe: bool,
    pub merge: bool,
    pub stylesheet: Option<PathBuf>,
    pub stylesheet_base: Option<PathBuf>,
    pub config_file: Option<PathBuf>,
    pub config_base: Option<PathBuf>,
}

impl Config {
    pub fn list(value: &str) -> Vec<String> {
        value
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect()
    }
}
