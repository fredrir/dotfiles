use std::path::{Component, Path};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    Auto,
    Generic,
    Git,
    EditorConfig,
    Systemd,
    Ssh,
}

impl Dialect {
    pub const ALL: [Self; 6] = [
        Self::Auto,
        Self::Generic,
        Self::Git,
        Self::EditorConfig,
        Self::Systemd,
        Self::Ssh,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Generic => "generic",
            Self::Git => "git",
            Self::EditorConfig => "editorconfig",
            Self::Systemd => "systemd",
            Self::Ssh => "ssh",
        }
    }

    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Auto => &[],
            Self::Generic => &["ini", "conf"],
            Self::Git => &["gitconfig", "git-config"],
            Self::EditorConfig => &["editor-config"],
            Self::Systemd => &["unit"],
            Self::Ssh => &["sshconfig", "ssh-config"],
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
            .ok_or_else(|| format!("invalid dialect: {value}"))
    }

    pub fn for_path(path: &Path) -> Option<Self> {
        let basename = path
            .file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase());
        if basename
            .as_deref()
            .is_some_and(|name| matches!(name, ".gitconfig" | ".gitmodules"))
            || ends_with_components(path, &[".git", "config"])
            || ends_with_components(path, &["git", "config"])
        {
            return Some(Self::Git);
        }
        if basename.as_deref() == Some(".editorconfig") {
            return Some(Self::EditorConfig);
        }
        if path
            .extension()
            .is_some_and(|extension| is_systemd_extension(&extension.to_string_lossy()))
        {
            return Some(Self::Systemd);
        }
        if basename
            .as_deref()
            .is_some_and(|name| matches!(name, "ssh_config" | "sshd_config"))
            || ends_with_components(path, &["ssh", "config"])
            || ends_with_components(path, &[".ssh", "config"])
            || parent_ends_with_components(path, &["ssh", "config.d"])
            || parent_ends_with_components(path, &[".ssh", "config.d"])
        {
            return Some(Self::Ssh);
        }
        None
    }

    pub fn resolve(self, configured: Self, path: &Path) -> Self {
        if self != Self::Auto {
            self
        } else if configured != Self::Auto {
            configured
        } else {
            Self::for_path(path).unwrap_or(Self::Generic)
        }
    }
}

impl FromStr for Dialect {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value, true)
    }
}

fn is_systemd_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "service"
            | "timer"
            | "path"
            | "socket"
            | "target"
            | "mount"
            | "automount"
            | "swap"
            | "slice"
            | "scope"
            | "device"
            | "network"
            | "netdev"
            | "link"
    )
}

fn ends_with_components(path: &Path, suffix: &[&str]) -> bool {
    let components = components(path);
    components.ends_with(
        &suffix
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>(),
    )
}

fn parent_ends_with_components(path: &Path, suffix: &[&str]) -> bool {
    path.parent()
        .is_some_and(|parent| ends_with_components(parent, suffix))
}

fn components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}
