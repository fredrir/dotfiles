use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Language {
    Bib,
    Conf,
    Json,
    Latex,
    Lua,
    Markdown,
    Toml,
    Yaml,
}

impl Language {
    pub const ALL: [Self; 8] = [
        Self::Bib,
        Self::Conf,
        Self::Json,
        Self::Latex,
        Self::Lua,
        Self::Markdown,
        Self::Toml,
        Self::Yaml,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Bib => "bib",
            Self::Conf => "conf",
            Self::Json => "json",
            Self::Latex => "latex",
            Self::Lua => "lua",
            Self::Markdown => "markdown",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
        }
    }

    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Bib => &["bib", "bibtex", "biblatex"],
            Self::Conf => &["conf"],
            Self::Json => &["json"],
            Self::Latex => &["latex", "tex"],
            Self::Lua => &["lua"],
            Self::Markdown => &["markdown", "md"],
            Self::Toml => &["toml"],
            Self::Yaml => &["yaml", "yml"],
        }
    }

    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Bib => &["bib"],
            Self::Conf => &["conf", "config", "dotfile"],
            Self::Json => &[
                "json",
                "jsonc",
                "hujson",
                "jwcc",
                "jsonl",
                "ndjson",
                "jsonlines",
            ],
            Self::Latex => &[
                "tex", "ltx", "latex", "sty", "cls", "dtx", "ins", "def", "clo", "lco", "fd",
                "bbx", "cbx", "lbx", "pgf", "tikz", "ctx", "mkii", "mkiv", "mkvi", "mkxl",
            ],
            Self::Lua => &["lua", "luau"],
            Self::Markdown => &["md", "markdown", "mdown", "mkd"],
            Self::Toml => &["toml"],
            Self::Yaml => &["yaml", "yml"],
        }
    }

    pub fn default_stdin(self) -> &'static Path {
        Path::new(match self {
            Self::Bib => "stdin.bib",
            Self::Conf => "stdin.conf",
            Self::Json => "stdin.json",
            Self::Latex => "stdin.tex",
            Self::Lua => "stdin.lua",
            Self::Markdown => "stdin.md",
            Self::Toml => "stdin.toml",
            Self::Yaml => "stdin.yaml",
        })
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|language| language.aliases().contains(&value))
            .ok_or_else(|| {
                format!(
                    "unknown language '{value}'; expected bib (bibtex, biblatex), conf, json, latex (tex), lua, markdown (md), toml, or yaml (yml)"
                )
            })
    }

    pub fn for_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        Self::ALL.into_iter().find(|language| {
            language
                .extensions()
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
    }
}
