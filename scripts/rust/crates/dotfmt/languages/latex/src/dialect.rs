use std::path::Path;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    Auto,
    Latex,
    Plain,
    Context,
}

pub struct Defaults {
    pub lists: &'static [&'static str],
    pub verbatims: &'static [&'static str],
    pub no_indent_envs: &'static [&'static str],
}

impl Dialect {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Latex, Self::Plain, Self::Context];

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Latex => "latex",
            Self::Plain => "plain",
            Self::Context => "context",
        }
    }

    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Auto => &[],
            Self::Latex => &[
                "latex2e", "latex3", "expl3", "pdflatex", "pdftex", "xelatex", "xetex", "lualatex",
                "luatex", "beamer", "tikz", "pgf",
            ],
            Self::Plain => &["plaintex", "tex", "initex", "optex", "eplain"],
            Self::Context => &["ctx", "mkii", "mkiv", "mkxl", "lmtx", "luametatex"],
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

    /// Select the dialect for an extension that names one, otherwise LaTeX.
    pub fn for_path(path: &Path) -> Option<Self> {
        Some(
            match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
                "ctx" | "mkii" | "mkiv" | "mkvi" | "mkxl" => Self::Context,
                _ => Self::Latex,
            },
        )
    }

    pub fn resolve(self, configured: Self, path: &Path) -> Self {
        if self != Self::Auto {
            self
        } else if configured != Self::Auto {
            configured
        } else {
            Self::for_path(path).unwrap_or(Self::Latex)
        }
    }

    pub fn defaults(self) -> Defaults {
        match self {
            Self::Auto | Self::Latex => Defaults {
                lists: &[
                    "itemize",
                    "enumerate",
                    "description",
                    "inlineroman",
                    "inventory",
                    "compactitem",
                    "compactenum",
                    "compactdesc",
                    "asparaitem",
                    "asparaenum",
                    "asparadesc",
                ],
                verbatims: &[
                    "verbatim",
                    "verbatim*",
                    "Verbatim",
                    "Verbatim*",
                    "BVerbatim",
                    "LVerbatim",
                    "lstlisting",
                    "minted",
                    "comment",
                    "filecontents",
                    "filecontents*",
                    "luacode",
                    "luacode*",
                    "pycode",
                    "Rcode",
                    "Sagecode",
                    "bashcode",
                    "shcode",
                    "asy",
                    "spverbatim",
                    "boxedverbatim",
                ],
                no_indent_envs: &["document"],
            },
            Self::Plain => Defaults {
                lists: &[],
                verbatims: &["verbatim", "comment"],
                no_indent_envs: &[],
            },
            Self::Context => Defaults {
                lists: &[
                    "itemize",
                    "enumeration",
                    "description",
                    "itemgroup",
                    "list",
                    "combination",
                ],
                verbatims: &[
                    "typing",
                    "TEX",
                    "MPcode",
                    "MPgraphic",
                    "Lua",
                    "buffer",
                    "verbatim",
                    "code",
                ],
                no_indent_envs: &["text", "document"],
            },
        }
    }
}

impl FromStr for Dialect {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value, false)
    }
}
