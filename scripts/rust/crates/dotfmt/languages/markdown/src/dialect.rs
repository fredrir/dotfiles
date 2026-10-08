use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    Auto,
    Commonmark,
    Gfm,
    Obsidian,
}

impl Dialect {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Commonmark, Self::Gfm, Self::Obsidian];

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Commonmark => "commonmark",
            Self::Gfm => "gfm",
            Self::Obsidian => "obsidian",
        }
    }

    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Auto => &[],
            Self::Commonmark => &[],
            Self::Gfm => &["github", "github-flavored-markdown"],
            Self::Obsidian => &["obsidian-markdown"],
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
}

impl FromStr for Dialect {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value, false)
    }
}
