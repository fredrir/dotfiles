use std::path::Path;

use malva::Syntax;

use crate::config::Config;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    Auto,
    Css,
    Scss,
    Sass,
    Less,
}

impl Dialect {
    pub const ALL: [Self; 5] = [Self::Auto, Self::Css, Self::Scss, Self::Sass, Self::Less];

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Css => "css",
            Self::Scss => "scss",
            Self::Sass => "sass",
            Self::Less => "less",
        }
    }

    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Auto => &["auto"],
            Self::Css => &["css", "pcss"],
            Self::Scss => &["scss"],
            Self::Sass => &["sass"],
            Self::Less => &["less"],
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
                    "unknown css dialect '{value}'; expected {}",
                    Self::ALL
                        .iter()
                        .map(|dialect| dialect.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }

    pub fn for_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "css" | "pcss" => Some(Self::Css),
            "scss" => Some(Self::Scss),
            "sass" => Some(Self::Sass),
            "less" => Some(Self::Less),
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
        Self::for_path(path).unwrap_or(Self::Css)
    }

    pub fn syntax(self) -> Syntax {
        match self {
            Self::Auto | Self::Css => Syntax::Css,
            Self::Scss => Syntax::Scss,
            Self::Sass => Syntax::Sass,
            Self::Less => Syntax::Less,
        }
    }
}

impl Config {
    pub fn resolved_dialect(&self, path: &Path) -> Dialect {
        Dialect::for_path(path).unwrap_or(Dialect::Css)
    }
}
