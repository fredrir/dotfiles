#[cfg(feature = "ratatui")]
use crate::ColorMode;
use crate::{Color, ColorDepth};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Role {
    #[default]
    Plain,
    Strong,
    Muted,
    Accent,
    Success,
    Warning,
    Danger,
    Info,
    Border,
    Selection,
    DiffAdded,
    DiffRemoved,
    DiffContext,
    DiffHeader,
    Ours,
    Theirs,
    Conflict,
    Background,
    Panel,
    Surface,
    Focus,
}

impl Role {
    pub const ALL: [Self; 21] = [
        Self::Plain,
        Self::Strong,
        Self::Muted,
        Self::Accent,
        Self::Success,
        Self::Warning,
        Self::Danger,
        Self::Info,
        Self::Border,
        Self::Selection,
        Self::DiffAdded,
        Self::DiffRemoved,
        Self::DiffContext,
        Self::DiffHeader,
        Self::Ours,
        Self::Theirs,
        Self::Conflict,
        Self::Background,
        Self::Panel,
        Self::Surface,
        Self::Focus,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Plain | Self::Strong => "foreground",
            Self::Muted => "muted",
            Self::Accent => "accent",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Danger => "danger",
            Self::Info => "info",
            Self::Border => "border",
            Self::Selection => "selection_foreground",
            Self::DiffAdded => "diff_added",
            Self::DiffRemoved => "diff_removed",
            Self::DiffContext => "diff_context",
            Self::DiffHeader => "diff_header",
            Self::Ours => "ours",
            Self::Theirs => "theirs",
            Self::Conflict => "conflict",
            Self::Background => "background",
            Self::Panel => "panel",
            Self::Surface => "surface",
            Self::Focus => "focus",
        }
    }

    pub(crate) fn bold(self) -> bool {
        matches!(
            self,
            Self::Strong | Self::Selection | Self::DiffHeader | Self::Conflict | Self::Focus
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteDocument {
    pub version: u32,
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub dark: bool,
    #[serde(default)]
    pub colors: BTreeMap<String, String>,
    #[serde(default)]
    pub roles: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ui: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ui_indexed: BTreeMap<String, u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub profile: String,
    pub dark: bool,
    document: PaletteDocument,
    colors: [Color; Role::ALL.len()],
    backgrounds: [Color; Role::ALL.len()],
    foregrounds: [Color; Role::ALL.len()],
    rendered_foregrounds: [Color; Role::ALL.len()],
    rendered_backgrounds: [Color; Role::ALL.len()],
    depth: ColorDepth,
}

impl Default for Palette {
    fn default() -> Self {
        let colors = Role::ALL.map(|role| match role {
            Role::Muted | Role::Border => Color::Ansi(8),
            Role::Accent | Role::Ours | Role::DiffHeader => Color::Ansi(5),
            Role::Success | Role::DiffAdded => Color::Ansi(2),
            Role::Warning | Role::Conflict => Color::Ansi(3),
            Role::Danger | Role::DiffRemoved => Color::Ansi(1),
            Role::Info | Role::Theirs | Role::Focus => Color::Ansi(6),
            _ => Color::Terminal,
        });
        Self {
            profile: "terminal".into(),
            dark: true,
            document: PaletteDocument {
                version: 1,
                profile: "terminal".into(),
                dark: true,
                colors: [
                    ("fg", "#c0c0c0"),
                    ("muted", "#808080"),
                    ("separator", "#808080"),
                    ("green", "#008000"),
                    ("yellow", "#808000"),
                    ("red", "#800000"),
                ]
                .map(|(key, value)| (key.into(), value.into()))
                .into(),
                roles: [
                    ("section_system", "#000080"),
                    ("section_hardware", "#808000"),
                    ("section_desktop", "#800080"),
                ]
                .map(|(key, value)| (key.into(), value.into()))
                .into(),
                ui: BTreeMap::new(),
                ui_indexed: BTreeMap::new(),
            },
            colors,
            backgrounds: [Color::Terminal; Role::ALL.len()],
            foregrounds: colors,
            rendered_foregrounds: colors,
            rendered_backgrounds: [Color::Terminal; Role::ALL.len()],
            depth: ColorDepth::TrueColor,
        }
    }
}

impl Palette {
    pub fn current() -> Arc<Self> {
        static CURRENT: OnceLock<Arc<Palette>> = OnceLock::new();
        Arc::clone(CURRENT.get_or_init(|| Arc::new(Self::load())))
    }

    pub fn load() -> Self {
        crate::ThemeHandle::discover().palette().as_ref().clone()
    }

    pub fn from_path(path: &Path) -> Result<Self, String> {
        let metadata =
            std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
        if metadata.len() > 256 * 1024 {
            return Err(format!("{}: palette exceeds 256 KiB", path.display()));
        }
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Self::from_json(&source).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn from_json(source: &str) -> Result<Self, String> {
        let value =
            serde_json::from_str(source).map_err(|error| format!("invalid palette: {error}"))?;
        Self::from_value(&value)
    }

    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        if value["version"] != 1 {
            return Err("unsupported palette version".into());
        }
        let document: PaletteDocument = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid palette: {error}"))?;
        Self::from_document(document)
    }

    pub fn from_document(document: PaletteDocument) -> Result<Self, String> {
        if document.version != 1 {
            return Err("unsupported palette version".into());
        }
        if document.colors.is_empty() {
            return Err("missing palette colors".into());
        }
        if document.profile.chars().any(char::is_control) || document.profile.len() > 128 {
            return Err("invalid palette profile".into());
        }
        for (group, map) in [
            ("colors", &document.colors),
            ("roles", &document.roles),
            ("ui", &document.ui),
        ] {
            for (name, value) in map {
                Color::parse(value)
                    .map_err(|_| format!("invalid palette color: {group}.{name}"))?;
            }
        }
        let mut palette = Self::default();
        for role in Role::ALL {
            let legacy = match role {
                Role::Plain | Role::Strong | Role::DiffContext => "fg",
                Role::Muted => "muted",
                Role::Accent | Role::DiffHeader | Role::Ours => "magenta",
                Role::Success | Role::DiffAdded => "green",
                Role::Warning | Role::Conflict => "yellow",
                Role::Danger | Role::DiffRemoved => "red",
                Role::Info => "blue",
                Role::Theirs => "cyan",
                Role::Border => "separator",
                Role::Selection => "foreground",
                Role::Focus => "primary",
                Role::Background => "background",
                Role::Panel => "accent",
                Role::Surface => "surface",
            };
            if let Some(value) = document
                .ui
                .get(role.key())
                .or_else(|| document.colors.get(legacy))
            {
                palette.colors[role as usize] = Color::parse(value)?;
            }
        }
        palette.foregrounds = palette.colors;
        for (role, foreground, background) in [
            (Role::Background, "foreground", "background"),
            (Role::Panel, "panel_foreground", "panel"),
            (Role::Surface, "surface_foreground", "surface"),
            (
                Role::Selection,
                "selection_foreground",
                "selection_background",
            ),
        ] {
            palette.foregrounds[role as usize] = document
                .ui
                .get(foreground)
                .map(|value| Color::parse(value))
                .transpose()?
                .unwrap_or(palette.colors[Role::Plain as usize]);
            palette.backgrounds[role as usize] = document
                .ui
                .get(background)
                .map(|value| Color::parse(value))
                .transpose()?
                .unwrap_or(Color::Terminal);
        }
        palette.profile = if document.profile.is_empty() {
            "custom".into()
        } else {
            document.profile.clone()
        };
        palette.dark = document.dark;
        palette.document = document;
        Ok(palette.with_depth(ColorDepth::detect()))
    }

    pub fn with_depth(mut self, depth: ColorDepth) -> Self {
        self.depth = depth;
        for role in Role::ALL {
            let foreground = match role {
                Role::Background => "foreground",
                Role::Panel => "panel_foreground",
                Role::Surface => "surface_foreground",
                _ => role.key(),
            };
            self.rendered_foregrounds[role as usize] =
                self.at_depth(foreground, self.foregrounds[role as usize], depth);
            let background = match role {
                Role::Background => "background",
                Role::Panel => "panel",
                Role::Surface => "surface",
                Role::Selection => "selection_background",
                _ => "",
            };
            self.rendered_backgrounds[role as usize] =
                self.at_depth(background, self.backgrounds[role as usize], depth);
        }
        self
    }

    fn at_depth(&self, name: &str, color: Color, depth: ColorDepth) -> Color {
        if depth == ColorDepth::Ansi256
            && let Some(index) = self.document.ui_indexed.get(name)
        {
            return Color::Ansi(*index);
        }
        color.at_depth(depth)
    }

    pub fn color(&self, role: Role) -> Color {
        self.colors[role as usize]
    }

    pub fn foreground(&self, role: Role) -> Color {
        self.rendered_foregrounds[role as usize]
    }

    pub fn background(&self, role: Role) -> Color {
        self.rendered_backgrounds[role as usize]
    }

    pub fn document(&self) -> &PaletteDocument {
        &self.document
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(&self.document).map_err(|error| error.to_string())
    }

    pub fn named_color(&self, group: &str, name: &str) -> Result<Color, String> {
        let map = match group {
            "colors" => &self.document.colors,
            "roles" => &self.document.roles,
            "ui" => &self.document.ui,
            _ => return Err(format!("unknown palette group: {group}")),
        };
        Color::parse(
            map.get(name)
                .ok_or_else(|| format!("missing palette {group}.{name}"))?,
        )
    }

    #[cfg(feature = "ratatui")]
    pub fn style(&self, role: Role) -> ratatui::style::Style {
        self.ratatui(ColorMode::Always, true, role)
    }

    #[cfg(feature = "ratatui")]
    pub fn ratatui(&self, mode: ColorMode, terminal: bool, role: Role) -> ratatui::style::Style {
        use ratatui::style::{Modifier, Style};
        let colored = mode.enabled(terminal);
        let mut style = Style::default();
        if colored {
            style = style.fg(self.foreground(role).ratatui());
            if self.background(role) != Color::Terminal {
                style = style.bg(self.background(role).ratatui());
            }
        }
        if role.bold() {
            style = style.add_modifier(Modifier::BOLD);
        }
        if role == Role::Selection && (!colored || self.background(role) == Color::Terminal) {
            style = style.add_modifier(Modifier::REVERSED);
        }
        style
    }
}

#[cfg(test)]
#[path = "../tests/unit/palette.rs"]
mod tests;
