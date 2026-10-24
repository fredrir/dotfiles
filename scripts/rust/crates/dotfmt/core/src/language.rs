use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Language {
    Conf,
    Json,
    Lua,
    Markdown,
    Toml,
    Yaml,
}

impl Language {
    pub const ALL: [Self; 6] = [
        Self::Conf,
        Self::Json,
        Self::Lua,
        Self::Markdown,
        Self::Toml,
        Self::Yaml,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Conf => "conf",
            Self::Json => "json",
            Self::Lua => "lua",
            Self::Markdown => "markdown",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
        }
    }

    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Conf => &["conf"],
            Self::Json => &["json"],
            Self::Lua => &["lua"],
            Self::Markdown => &["markdown", "md"],
            Self::Toml => &["toml"],
            Self::Yaml => &["yaml", "yml"],
        }
    }

    pub fn extensions(self) -> &'static [&'static str] {
        match self {
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
            Self::Lua => &["lua", "luau"],
            Self::Markdown => &["md", "markdown", "mdown", "mkd"],
            Self::Toml => &["toml"],
            Self::Yaml => &["yaml", "yml"],
        }
    }

    pub fn default_stdin(self) -> &'static Path {
        Path::new(match self {
            Self::Conf => "stdin.conf",
            Self::Json => "stdin.json",
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
                    "unknown language '{value}'; expected conf, json, lua, markdown (md), toml, or yaml (yml)"
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
