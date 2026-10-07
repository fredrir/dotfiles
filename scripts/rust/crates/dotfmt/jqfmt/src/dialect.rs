use std::path::Path;

use clap::ValueEnum;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Dialect {
    #[default]
    Auto,
    Json,
    #[value(alias = "json-with-comments")]
    Jsonc,
    #[value(alias = "jwcc")]
    Hujson,
}

impl Dialect {
    pub fn for_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "jsonc" => Some(Self::Jsonc),
            "hujson" | "jwcc" => Some(Self::Hujson),
            _ => None,
        }
    }

    pub fn resolve(self, path: &Path) -> Self {
        match self {
            Self::Auto => Self::for_path(path).unwrap_or(Self::Json),
            explicit => explicit,
        }
    }

    pub fn comments(self) -> bool {
        matches!(self, Self::Jsonc | Self::Hujson)
    }
}
