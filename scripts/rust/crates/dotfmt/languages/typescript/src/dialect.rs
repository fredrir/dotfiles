use std::path::Path;

use oxc_span::{FileExtension, SourceType};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    Auto,
    Ts,
    Tsx,
    Js,
    Jsx,
}

impl Dialect {
    pub const ALL: [Self; 5] = [Self::Auto, Self::Ts, Self::Tsx, Self::Js, Self::Jsx];

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ts => "ts",
            Self::Tsx => "tsx",
            Self::Js => "js",
            Self::Jsx => "jsx",
        }
    }

    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Auto => &["auto"],
            Self::Ts => &["ts", "typescript"],
            Self::Tsx => &["tsx"],
            Self::Js => &["js", "javascript"],
            Self::Jsx => &["jsx"],
        }
    }

    pub fn parse(value: &str, ignore_case: bool) -> Result<Self, String> {
        let matches = |candidate: &str| {
            if ignore_case {
                candidate.eq_ignore_ascii_case(value)
            } else {
                candidate == value
            }
        };
        Self::ALL
            .into_iter()
            .find(|dialect| dialect.aliases().iter().any(|alias| matches(alias)))
            .ok_or_else(|| {
                format!(
                    "unknown typescript dialect '{value}'; expected {}",
                    Self::ALL
                        .iter()
                        .map(|dialect| dialect.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }

    pub fn for_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        match extension.to_ascii_lowercase().as_str() {
            "ts" | "mts" | "cts" => Some(Self::Ts),
            "tsx" => Some(Self::Tsx),
            "js" | "mjs" | "cjs" => Some(Self::Js),
            "jsx" => Some(Self::Jsx),
            _ => None,
        }
    }

    pub fn resolve(self, configured: Self, path: &Path) -> Self {
        if configured != Self::Auto {
            return configured;
        }
        if self != Self::Auto {
            return self;
        }
        Self::for_path(path).unwrap_or(Self::Ts)
    }

    pub fn source_type(self) -> SourceType {
        match self {
            Self::Auto | Self::Ts => SourceType::ts(),
            Self::Tsx => SourceType::tsx(),
            Self::Js => SourceType::unambiguous(),
            Self::Jsx => SourceType::jsx(),
        }
    }

    pub fn source_type_for_path(self, path: &Path) -> SourceType {
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".d.ts"))
        {
            return SourceType::d_ts();
        }
        if self != Self::Auto {
            return self.source_type();
        }
        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            return SourceType::ts();
        };
        let extension = extension.to_ascii_lowercase();
        extension
            .parse::<FileExtension>()
            .map(SourceType::from)
            .unwrap_or_else(|_| SourceType::ts())
    }
}
