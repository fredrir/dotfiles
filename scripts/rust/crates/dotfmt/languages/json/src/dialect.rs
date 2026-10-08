use std::path::Path;

use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    Auto,
    Json,
    Jsonc,
    Hujson,
}

impl Dialect {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Json, Self::Jsonc, Self::Hujson];

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Json => "json",
            Self::Jsonc => "jsonc",
            Self::Hujson => "hujson",
        }
    }

    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Auto => &[],
            Self::Json => &[],
            Self::Jsonc => &["json-with-comments"],
            Self::Hujson => &["jwcc"],
        }
    }

    pub fn parse(value: &str, ignore_case: bool) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|dialect| {
                std::iter::once(dialect.name())
                    .chain(dialect.aliases().iter().copied())
                    .any(|name| {
                        if ignore_case {
                            name.eq_ignore_ascii_case(value)
                        } else {
                            name == value
                        }
                    })
            })
            .ok_or_else(|| format!("invalid variant: {value}"))
    }

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

impl FromStr for Dialect {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value, false)
    }
}
