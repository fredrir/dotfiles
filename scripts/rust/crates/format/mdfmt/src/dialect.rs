use std::path::Path;

use clap::ValueEnum;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Dialect {
    #[default]
    Auto,
    Commonmark,
    #[value(alias = "github", alias = "github-flavored-markdown")]
    Gfm,
    #[value(alias = "obsidian-markdown")]
    Obsidian,
}

impl Dialect {
    pub fn resolve(self, configured: Self, path: &Path) -> Self {
        if self != Self::Auto {
            return self;
        }
        if configured != Self::Auto {
            return configured;
        }
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if path
            .ancestors()
            .skip(1)
            .any(|directory| directory.join(".obsidian").is_dir())
        {
            Self::Obsidian
        } else {
            Self::Gfm
        }
    }
}
