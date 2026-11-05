use std::path::Path;

use markup_fmt::Language;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    Auto,
    Html,
    Vue,
    Svelte,
    Astro,
    Angular,
    Xml,
}

impl Dialect {
    pub const ALL: [Self; 7] = [
        Self::Auto,
        Self::Html,
        Self::Vue,
        Self::Svelte,
        Self::Astro,
        Self::Angular,
        Self::Xml,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Html => "html",
            Self::Vue => "vue",
            Self::Svelte => "svelte",
            Self::Astro => "astro",
            Self::Angular => "angular",
            Self::Xml => "xml",
        }
    }

    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Auto => &["auto"],
            Self::Html => &["html", "htm"],
            Self::Vue => &["vue"],
            Self::Svelte => &["svelte"],
            Self::Astro => &["astro"],
            Self::Angular => &["angular"],
            Self::Xml => &["xml", "svg"],
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
                    "unknown html dialect '{value}'; expected {}",
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
            "html" | "htm" | "xhtml" => Some(Self::Html),
            "vue" => Some(Self::Vue),
            "svelte" => Some(Self::Svelte),
            "astro" => Some(Self::Astro),
            "component.html" => Some(Self::Angular),
            "xml" | "svg" | "xsd" | "xsl" | "xslt" => Some(Self::Xml),
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
        Self::for_path(path).unwrap_or(Self::Html)
    }

    pub fn language(self) -> Language {
        match self {
            Self::Auto | Self::Html => Language::Html,
            Self::Vue => Language::Vue,
            Self::Svelte => Language::Svelte,
            Self::Astro => Language::Astro,
            Self::Angular => Language::Angular,
            Self::Xml => Language::Xml,
        }
    }
}
