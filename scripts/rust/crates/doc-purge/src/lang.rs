use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialect {
    Rust,
    Go,
    C,
    Cpp,
    Java,
    Kotlin,
    Swift,
    JavaScript,
    TypeScript,
    Jsx,
    CSharp,
    Zig,
    Python,
    Ruby,
    Shell,
    Yaml,
    Toml,
    Lua,
    Sql,
    Haskell,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    Curly,
    Hash,
    Dash,
}

pub struct Language {
    pub name: &'static str,
    pub dialect: Dialect,
    pub extensions: &'static [&'static str],
}

impl Dialect {
    pub fn family(self) -> Family {
        match self {
            Dialect::Python | Dialect::Ruby | Dialect::Shell | Dialect::Yaml | Dialect::Toml => {
                Family::Hash
            }
            Dialect::Lua | Dialect::Sql | Dialect::Haskell => Family::Dash,
            _ => Family::Curly,
        }
    }
}

pub const LANGUAGES: &[Language] = &[
    Language {
        name: "rust",
        dialect: Dialect::Rust,
        extensions: &["rs"],
    },
    Language {
        name: "go",
        dialect: Dialect::Go,
        extensions: &["go"],
    },
    Language {
        name: "c",
        dialect: Dialect::C,
        extensions: &["c", "h"],
    },
    Language {
        name: "cpp",
        dialect: Dialect::Cpp,
        extensions: &["cc", "cpp", "cxx", "hh", "hpp", "hxx"],
    },
    Language {
        name: "java",
        dialect: Dialect::Java,
        extensions: &["java"],
    },
    Language {
        name: "kotlin",
        dialect: Dialect::Kotlin,
        extensions: &["kt", "kts"],
    },
    Language {
        name: "swift",
        dialect: Dialect::Swift,
        extensions: &["swift"],
    },
    Language {
        name: "javascript",
        dialect: Dialect::JavaScript,
        extensions: &["js", "mjs", "cjs"],
    },
    Language {
        name: "typescript",
        dialect: Dialect::TypeScript,
        extensions: &["ts", "mts", "cts"],
    },
    Language {
        name: "jsx",
        dialect: Dialect::Jsx,
        extensions: &["jsx", "tsx"],
    },
    Language {
        name: "csharp",
        dialect: Dialect::CSharp,
        extensions: &["cs"],
    },
    Language {
        name: "zig",
        dialect: Dialect::Zig,
        extensions: &["zig"],
    },
    Language {
        name: "python",
        dialect: Dialect::Python,
        extensions: &["py", "pyi"],
    },
    Language {
        name: "ruby",
        dialect: Dialect::Ruby,
        extensions: &["rb"],
    },
    Language {
        name: "shell",
        dialect: Dialect::Shell,
        extensions: &["sh", "bash", "zsh"],
    },
    Language {
        name: "yaml",
        dialect: Dialect::Yaml,
        extensions: &["yaml", "yml"],
    },
    Language {
        name: "toml",
        dialect: Dialect::Toml,
        extensions: &["toml"],
    },
    Language {
        name: "lua",
        dialect: Dialect::Lua,
        extensions: &["lua"],
    },
    Language {
        name: "sql",
        dialect: Dialect::Sql,
        extensions: &["sql"],
    },
    Language {
        name: "haskell",
        dialect: Dialect::Haskell,
        extensions: &["hs"],
    },
];

pub fn for_path(path: &Path) -> Option<&'static Language> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    LANGUAGES
        .iter()
        .find(|language| language.extensions.contains(&extension.as_str()))
}

pub fn for_token(token: &str) -> Option<&'static Language> {
    let wanted = token.trim().trim_start_matches('.').to_ascii_lowercase();
    if wanted.is_empty() {
        return None;
    }
    LANGUAGES
        .iter()
        .find(|language| language.name == wanted || language.extensions.contains(&wanted.as_str()))
}

pub fn known() -> String {
    let mut every: Vec<&str> = LANGUAGES
        .iter()
        .flat_map(|language| language.extensions.iter().copied())
        .collect();
    every.sort_unstable();
    every.join(" ")
}
