use std::path::Path;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    Auto,
    Git,
    Ignore,
    Prettier,
    Vscode,
    Attributes,
}

impl Dialect {
    pub const ALL: [Self; 6] = [
        Self::Auto,
        Self::Git,
        Self::Ignore,
        Self::Prettier,
        Self::Vscode,
        Self::Attributes,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Git => "git",
            Self::Ignore => "ignore",
            Self::Prettier => "prettier",
            Self::Vscode => "vscode",
            Self::Attributes => "attributes",
        }
    }

    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Auto => &[],
            Self::Git => &["gitignore"],
            Self::Ignore => &["ripgrep"],
            Self::Prettier => &["prettierignore"],
            Self::Vscode => &["vscodeignore"],
            Self::Attributes => &["gitattributes", "attr"],
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

    pub fn resolve(self, configured: Self, detected: Self) -> Self {
        if self != Self::Auto {
            self
        } else if configured != Self::Auto {
            configured
        } else {
            detected
        }
    }

    pub fn detect(path: &Path) -> Self {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return match path.extension().and_then(|extension| extension.to_str()) {
                Some("gitattributes") => Self::Attributes,
                Some("ignore") => Self::Ignore,
                _ => Self::Git,
            };
        };
        match name {
            ".gitignore" => Self::Git,
            ".ignore" => Self::Ignore,
            "ignore" => Self::Git,
            ".prettierignore" => Self::Prettier,
            ".vscodeignore" => Self::Vscode,
            ".gitattributes" => Self::Attributes,
            _ if name.ends_with(".gitignore") => Self::Git,
            _ if name.ends_with(".ignore") => Self::Ignore,
            _ if name.ends_with(".gitattributes") => Self::Attributes,
            _ if is_info_file(path, "exclude") => Self::Git,
            _ if is_info_file(path, "attributes") => Self::Attributes,
            _ => Self::Git,
        }
    }

    pub(crate) fn base(self) -> Self {
        if self == Self::Auto { Self::Git } else { self }
    }
}

fn is_info_file(path: &Path, name: &str) -> bool {
    path.file_name().and_then(|file| file.to_str()) == Some(name)
        && path
            .parent()
            .and_then(Path::file_name)
            .and_then(|parent| parent.to_str())
            == Some("info")
}

impl FromStr for Dialect {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value, false)
    }
}
