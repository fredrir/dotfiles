use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Language {
    Bib,
    Conf,
    Env,
    Ignore,
    Ini,
    Json,
    Latex,
    Lua,
    Markdown,
    Plist,
    Toml,
    Yaml,
    TypeScript,
    Javascript,
    Css,
    Html,
}

impl Language {
    pub const ALL: [Self; 16] = [
        Self::Bib,
        Self::Conf,
        Self::Env,
        Self::Ignore,
        Self::Ini,
        Self::Json,
        Self::Latex,
        Self::Lua,
        Self::Markdown,
        Self::Plist,
        Self::Toml,
        Self::Yaml,
        Self::TypeScript,
        Self::Javascript,
        Self::Css,
        Self::Html,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Bib => "bib",
            Self::Conf => "conf",
            Self::Env => "env",
            Self::Ignore => "ignore",
            Self::Ini => "ini",
            Self::Json => "json",
            Self::Latex => "latex",
            Self::Lua => "lua",
            Self::Markdown => "markdown",
            Self::Plist => "plist",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
            Self::TypeScript => "typescript",
            Self::Javascript => "javascript",
            Self::Css => "css",
            Self::Html => "html",
        }
    }

    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Bib => &["bib", "bibtex", "biblatex"],
            Self::Conf => &["conf"],
            Self::Env => &["dotenv", "env"],
            Self::Ignore => &["gitignore", "ignore"],
            Self::Ini => &["ini"],
            Self::Json => &["json"],
            Self::Latex => &["latex", "tex"],
            Self::Lua => &["lua"],
            Self::Markdown => &["markdown", "md"],
            Self::Plist => &["plist"],
            Self::Toml => &["toml"],
            Self::Yaml => &["yaml", "yml"],
            Self::TypeScript => &["typescript", "ts", "tsx"],
            Self::Javascript => &["javascript", "js", "jsx"],
            Self::Css => &["css", "scss", "sass", "less"],
            Self::Html => &["html", "htm", "vue", "svelte", "astro"],
        }
    }

    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Bib => &["bib"],
            Self::Conf => &["conf", "config", "dotfile"],
            Self::Env => &["env"],
            Self::Ignore => &["gitignore", "gitattributes", "ignore"],
            Self::Ini => &[
                "ini",
                "service",
                "timer",
                "path",
                "socket",
                "target",
                "mount",
                "automount",
                "swap",
                "slice",
                "scope",
                "device",
                "network",
                "netdev",
                "link",
            ],
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
            Self::Plist => &["plist"],
            Self::Toml => &["toml"],
            Self::Yaml => &["yaml", "yml"],
            Self::TypeScript => &["ts", "tsx", "mts", "cts"],
            Self::Javascript => &["js", "jsx", "mjs", "cjs"],
            Self::Css => &["css", "scss", "sass", "less", "pcss"],
            Self::Html => &["html", "htm", "xhtml", "vue", "svelte", "astro"],
        }
    }

    /// Exact file names that are not extensions because they begin with a dot
    /// or are conventional config names.
    fn basenames(self) -> &'static [&'static str] {
        match self {
            Self::Env => &[".env", "env"],
            Self::Ignore => &[
                ".gitignore",
                ".ignore",
                "ignore",
                ".prettierignore",
                ".vscodeignore",
                ".gitattributes",
            ],
            Self::Ini => &[
                ".gitconfig",
                ".gitmodules",
                ".editorconfig",
                "ssh_config",
                "sshd_config",
            ],
            _ => &[],
        }
    }

    /// File-name prefixes, matched after exact names and before extensions.
    fn prefixes(self) -> &'static [&'static str] {
        match self {
            Self::Env => &[".env."],
            _ => &[],
        }
    }

    /// Path suffixes that identify a file by its parent directory as well as
    /// its name.
    fn path_suffixes(self) -> &'static [&'static str] {
        match self {
            Self::Ignore => &[".git/info/exclude", ".git/info/attributes"],
            Self::Ini => &[".git/config", "git/config", ".ssh/config", "ssh/config"],
            _ => &[],
        }
    }

    /// Directory suffixes whose whole contents belong to one language.
    fn path_dirs(self) -> &'static [&'static str] {
        match self {
            Self::Ini => &[".ssh/config.d", "ssh/config.d"],
            _ => &[],
        }
    }

    pub fn default_stdin(self) -> &'static Path {
        Path::new(match self {
            Self::Bib => "stdin.bib",
            Self::Conf => "stdin.conf",
            Self::Env => "stdin.env",
            Self::Ignore => "stdin.gitignore",
            Self::Ini => "stdin.ini",
            Self::Json => "stdin.json",
            Self::Latex => "stdin.tex",
            Self::Lua => "stdin.lua",
            Self::Markdown => "stdin.md",
            Self::Plist => "stdin.plist",
            Self::Toml => "stdin.toml",
            Self::Yaml => "stdin.yaml",
            Self::TypeScript => "stdin.ts",
            Self::Javascript => "stdin.js",
            Self::Css => "stdin.css",
            Self::Html => "stdin.html",
        })
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|language| language.aliases().contains(&value))
            .ok_or_else(|| {
                let expected = Self::ALL
                    .iter()
                    .map(|language| language.describe())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("unknown language '{value}'; expected {expected}")
            })
    }

    fn describe(self) -> String {
        let aliases = self
            .aliases()
            .iter()
            .copied()
            .filter(|alias| *alias != self.name())
            .collect::<Vec<_>>();
        if aliases.is_empty() {
            self.name().to_owned()
        } else {
            format!("{} ({})", self.name(), aliases.join(", "))
        }
    }

    pub fn for_path(path: &Path) -> Option<Self> {
        // A non-UTF-8 basename must not hide an extension that is valid UTF-8.
        let name = path.file_name().and_then(|name| name.to_str());
        if let Some(name) = name {
            if let Some(language) = Self::ALL
                .into_iter()
                .find(|language| language.basenames().contains(&name))
            {
                return Some(language);
            }
            if let Some(language) = Self::ALL.into_iter().find(|language| {
                language
                    .prefixes()
                    .iter()
                    .any(|prefix| name.starts_with(prefix) && name.len() > prefix.len())
            }) {
                return Some(language);
            }
        }
        if let Some(language) = Self::ALL.into_iter().find(|language| {
            language
                .path_suffixes()
                .iter()
                .any(|suffix| path.ends_with(Path::new(suffix)))
        }) {
            return Some(language);
        }
        if let Some(parent) = path.parent()
            && let Some(language) = Self::ALL.into_iter().find(|language| {
                language
                    .path_dirs()
                    .iter()
                    .any(|directory| parent.ends_with(Path::new(directory)))
            })
        {
            return Some(language);
        }
        let extension = path.extension()?.to_str()?;
        Self::ALL.into_iter().find(|language| {
            language
                .extensions()
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
    }
}
