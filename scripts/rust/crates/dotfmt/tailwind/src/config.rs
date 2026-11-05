use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    Off,
    On,
    /// Sort only when the file or its project looks like a Tailwind project.
    #[default]
    Auto,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OrderSource {
    /// Use the project's own Tailwind when available, otherwise the Rust fallback.
    #[default]
    Auto,
    /// Require the project sorter; report an error when Node or Tailwind is unavailable.
    Project,
    /// Always use the Rust pattern sorter.
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
    /// Directory of the configuration layer that declared `stylesheet`.
    /// `None` means a global setting, resolved against the workspace root.
    pub stylesheet_base: Option<PathBuf>,
    pub config_file: Option<PathBuf>,
    /// Directory of the configuration layer that declared `config_file`.
    /// `None` means a global setting, resolved against the workspace root.
    pub config_base: Option<PathBuf>,
}

impl Config {
    /// Tailwind's parser accepts a comma-separated value from `dotfmt.dotfile`.
    pub fn list(value: &str) -> Vec<String> {
        value
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect()
    }
}
