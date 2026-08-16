use dotfile_source::{ByteRange, RepoPath};

/// A validated value together with the source bytes that authored it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Spanned<T> {
    pub value: T,
    pub span: ByteRange,
}

impl<T> Spanned<T> {
    pub(crate) fn new(value: T, span: ByteRange) -> Self {
        Self { value, span }
    }
}

/// The eight path-selected theme source schemas.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThemeFileKind {
    Roles,
    Fonts,
    Profile,
    CatppuccinMap,
    EzaMap,
    GtkMap,
    KdeMap,
    ObsidianMap,
}

/// The filename-derived identity of a theme profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeIdentity {
    pub name: String,
    pub path: RepoPath,
}

/// One validated bare reference. Its namespace is supplied by its field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeReference {
    pub name: String,
    pub span: ByteRange,
}

/// One entry in an open role map, in source order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoleBinding {
    pub name: Spanned<String>,
    pub palette: ThemeReference,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoleMap {
    pub entries: Vec<RoleBinding>,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalRoles {
    pub direct: Vec<RoleBinding>,
    pub ansi: Option<RoleMap>,
    pub tabs: Option<RoleMap>,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EzaPattern {
    pub key: Spanned<String>,
    pub role: ThemeReference,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EzaRoles {
    pub direct: Vec<RoleBinding>,
    pub categories: Option<RoleMap>,
    pub patterns: Vec<EzaPattern>,
    pub span: ByteRange,
}

/// The file-local role tree. All root blocks are optional by schema.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeRoles {
    pub roles: Option<RoleMap>,
    pub terminal: Option<TerminalRoles>,
    pub eza: Option<EzaRoles>,
    pub kde: Option<RoleMap>,
    pub konsole: Option<RoleMap>,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontBinding {
    pub name: Spanned<String>,
    pub family: Spanned<String>,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontMap {
    pub entries: Vec<FontBinding>,
    pub span: ByteRange,
}

/// A canonical, non-negative decimal spelling. Field schemas additionally
/// constrain whether zero is permitted and the upper bound.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CanonicalDecimal(pub(crate) String);

impl CanonicalDecimal {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeSizes {
    pub terminal: Option<Spanned<CanonicalDecimal>>,
    pub terminal_mac: Option<Spanned<CanonicalDecimal>>,
    pub interface: Option<Spanned<CanonicalDecimal>>,
    pub span: ByteRange,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ApplicationState {
    Enabled,
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationSetting {
    pub name: Spanned<String>,
    pub state: Spanned<ApplicationState>,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationMap {
    pub entries: Vec<ApplicationSetting>,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeFonts {
    pub fonts: FontMap,
    pub sizes: ThemeSizes,
    pub applications: ApplicationMap,
    pub span: ByteRange,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Appearance {
    Dark,
    Light,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HexColor(pub(crate) String);

impl HexColor {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaletteBinding {
    pub name: Spanned<String>,
    pub color: Spanned<HexColor>,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Palette {
    pub entries: Vec<PaletteBinding>,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NvimSettings {
    pub flavour: Spanned<String>,
    pub span: ByteRange,
}

/// Sparse profile-local overrides. Merging these with the shared trees is an
/// M3 operation; this type only records validated contributions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThemeOverrides {
    pub roles: Option<RoleMap>,
    pub terminal: Option<TerminalRoles>,
    pub eza: Option<EzaRoles>,
    pub kde: Option<RoleMap>,
    pub konsole: Option<RoleMap>,
    pub fonts: Option<FontMap>,
    pub sizes: Option<ThemeSizes>,
    pub applications: Option<ApplicationMap>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeProfile {
    pub identity: ThemeIdentity,
    pub display_name: Spanned<String>,
    pub appearance: Spanned<Appearance>,
    pub icons: Spanned<String>,
    pub nvim: NvimSettings,
    pub palette: Palette,
    pub overrides: ThemeOverrides,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HexKey(pub(crate) String);

impl HexKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatppuccinEntry {
    pub key: Spanned<HexKey>,
    pub palette: ThemeReference,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatppuccinMap {
    pub entries: Vec<CatppuccinEntry>,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EzaExtension(pub(crate) String);

impl EzaExtension {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EzaCategory {
    pub name: Spanned<String>,
    pub extensions: Vec<Spanned<EzaExtension>>,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EzaMap {
    pub categories: Vec<EzaCategory>,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GtkEntry {
    pub key: Spanned<String>,
    pub role: ThemeReference,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GtkMap {
    pub entries: Vec<GtkEntry>,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KdeGroupEntry {
    pub key: Spanned<String>,
    pub roles: [ThemeReference; 2],
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KdeRoleEntry {
    pub key: Spanned<String>,
    pub role: ThemeReference,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KdeMap {
    pub groups: Vec<KdeGroupEntry>,
    pub foregrounds: Vec<KdeRoleEntry>,
    pub selection_foregrounds: Vec<KdeRoleEntry>,
    pub span: ByteRange,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ObsidianDerived {
    AccentH,
    AccentS,
    AccentL,
    AccentHsl,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObsidianValue {
    Palette(ThemeReference),
    Rgb(ThemeReference),
    Color {
        color: ThemeReference,
        alpha: Spanned<CanonicalDecimal>,
    },
    Derived(Spanned<ObsidianDerived>),
    Literal(Spanned<String>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObsidianVariable {
    pub key: Spanned<String>,
    pub value: ObsidianValue,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObsidianMap {
    pub source: ThemeReference,
    pub variables: Vec<ObsidianVariable>,
    pub span: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThemeMap {
    Catppuccin(CatppuccinMap),
    Eza(EzaMap),
    Gtk(GtkMap),
    Kde(KdeMap),
    Obsidian(ObsidianMap),
}

/// A fully validated immediate theme source. It deliberately contains no
/// cross-file resolution or merged/effective theme state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThemeDocument {
    Roles(ThemeRoles),
    Fonts(ThemeFonts),
    Profile(Box<ThemeProfile>),
    Map(ThemeMap),
}

pub(crate) fn dump_lowering(lowering: &crate::ThemeLowering) -> serde_json::Value {
    use serde_json::{Value, json};

    let kind = lowering
        .kind()
        .map(kind_name)
        .map(Value::from)
        .unwrap_or(Value::Null);
    let document = lowering
        .document()
        .map(dump_document)
        .unwrap_or(Value::Null);
    let hir = lowering
        .hir()
        .map(|hir| hir.dump_json(lowering.schema()))
        .unwrap_or(Value::Null);
    let mut result = json!({ "kind": kind, "document": document, "hir": hir });
    if lowering.has_errors() {
        result["validated"] = Value::Bool(false);
        result["partial_document"] = lowering
            .partial_document()
            .map(dump_document)
            .unwrap_or(Value::Null);
    }
    result
}

fn kind_name(kind: ThemeFileKind) -> &'static str {
    match kind {
        ThemeFileKind::Roles => "roles",
        ThemeFileKind::Fonts => "fonts",
        ThemeFileKind::Profile => "profile",
        ThemeFileKind::CatppuccinMap => "map/catppuccin",
        ThemeFileKind::EzaMap => "map/eza",
        ThemeFileKind::GtkMap => "map/gtk",
        ThemeFileKind::KdeMap => "map/kde",
        ThemeFileKind::ObsidianMap => "map/obsidian",
    }
}

fn span_json(span: ByteRange) -> serde_json::Value {
    serde_json::json!([span.start(), span.end()])
}

fn string_json(value: &Spanned<String>) -> serde_json::Value {
    serde_json::json!({ "value": value.value, "span": span_json(value.span) })
}

fn reference_json(value: &ThemeReference) -> serde_json::Value {
    serde_json::json!({ "name": value.name, "span": span_json(value.span) })
}

fn dump_document(document: &ThemeDocument) -> serde_json::Value {
    match document {
        ThemeDocument::Roles(roles) => serde_json::json!({
            "type": "roles",
            "roles": roles.roles.as_ref().map(dump_role_map),
            "terminal": roles.terminal.as_ref().map(dump_terminal),
            "eza": roles.eza.as_ref().map(dump_eza_roles),
            "kde": roles.kde.as_ref().map(dump_role_map),
            "konsole": roles.konsole.as_ref().map(dump_role_map),
            "span": span_json(roles.span),
        }),
        ThemeDocument::Fonts(fonts) => serde_json::json!({
            "type": "fonts",
            "fonts": dump_font_map(&fonts.fonts),
            "sizes": dump_sizes(&fonts.sizes),
            "applications": dump_applications(&fonts.applications),
            "span": span_json(fonts.span),
        }),
        ThemeDocument::Profile(profile) => serde_json::json!({
            "type": "profile",
            "identity": { "name": profile.identity.name, "path": profile.identity.path.as_str() },
            "display-name": string_json(&profile.display_name),
            "appearance": {
                "value": match profile.appearance.value { Appearance::Dark => "dark", Appearance::Light => "light" },
                "span": span_json(profile.appearance.span),
            },
            "icons": string_json(&profile.icons),
            "nvim": { "flavour": string_json(&profile.nvim.flavour), "span": span_json(profile.nvim.span) },
            "palette": profile.palette.entries.iter().map(|entry| serde_json::json!({
                "name": string_json(&entry.name),
                "color": { "value": entry.color.value.as_str(), "span": span_json(entry.color.span) },
                "span": span_json(entry.span),
            })).collect::<Vec<_>>(),
            "overrides": dump_overrides(&profile.overrides),
            "span": span_json(profile.span),
        }),
        ThemeDocument::Map(map) => dump_map(map),
    }
}

fn dump_role_map(map: &RoleMap) -> serde_json::Value {
    serde_json::json!({
        "entries": map.entries.iter().map(|entry| serde_json::json!({
            "name": string_json(&entry.name),
            "palette": reference_json(&entry.palette),
            "span": span_json(entry.span),
        })).collect::<Vec<_>>(),
        "span": span_json(map.span),
    })
}

fn dump_terminal(terminal: &TerminalRoles) -> serde_json::Value {
    serde_json::json!({
        "direct": dump_role_entries(&terminal.direct),
        "ansi": terminal.ansi.as_ref().map(dump_role_map),
        "tabs": terminal.tabs.as_ref().map(dump_role_map),
        "span": span_json(terminal.span),
    })
}

fn dump_eza_roles(eza: &EzaRoles) -> serde_json::Value {
    serde_json::json!({
        "direct": dump_role_entries(&eza.direct),
        "categories": eza.categories.as_ref().map(dump_role_map),
        "patterns": eza.patterns.iter().map(|pattern| serde_json::json!({
            "key": string_json(&pattern.key),
            "role": reference_json(&pattern.role),
            "span": span_json(pattern.span),
        })).collect::<Vec<_>>(),
        "span": span_json(eza.span),
    })
}

fn dump_role_entries(entries: &[RoleBinding]) -> Vec<serde_json::Value> {
    entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "name": string_json(&entry.name),
                "palette": reference_json(&entry.palette),
                "span": span_json(entry.span),
            })
        })
        .collect()
}

fn dump_font_map(map: &FontMap) -> serde_json::Value {
    serde_json::json!({
        "entries": map.entries.iter().map(|entry| serde_json::json!({
            "name": string_json(&entry.name),
            "family": string_json(&entry.family),
            "span": span_json(entry.span),
        })).collect::<Vec<_>>(),
        "span": span_json(map.span),
    })
}

fn dump_sizes(sizes: &ThemeSizes) -> serde_json::Value {
    serde_json::json!({
        "terminal": sizes.terminal.as_ref().map(decimal_json),
        "terminal_mac": sizes.terminal_mac.as_ref().map(decimal_json),
        "interface": sizes.interface.as_ref().map(decimal_json),
        "span": span_json(sizes.span),
    })
}

fn decimal_json(decimal: &Spanned<CanonicalDecimal>) -> serde_json::Value {
    serde_json::json!({ "value": decimal.value.as_str(), "span": span_json(decimal.span) })
}

fn dump_applications(applications: &ApplicationMap) -> serde_json::Value {
    serde_json::json!({
        "entries": applications.entries.iter().map(|entry| serde_json::json!({
            "name": string_json(&entry.name),
            "state": {
                "value": match entry.state.value { ApplicationState::Enabled => "enabled", ApplicationState::Disabled => "disabled" },
                "span": span_json(entry.state.span),
            },
            "span": span_json(entry.span),
        })).collect::<Vec<_>>(),
        "span": span_json(applications.span),
    })
}

fn dump_overrides(overrides: &ThemeOverrides) -> serde_json::Value {
    serde_json::json!({
        "roles": overrides.roles.as_ref().map(dump_role_map),
        "terminal": overrides.terminal.as_ref().map(dump_terminal),
        "eza": overrides.eza.as_ref().map(dump_eza_roles),
        "kde": overrides.kde.as_ref().map(dump_role_map),
        "konsole": overrides.konsole.as_ref().map(dump_role_map),
        "fonts": overrides.fonts.as_ref().map(dump_font_map),
        "sizes": overrides.sizes.as_ref().map(dump_sizes),
        "applications": overrides.applications.as_ref().map(dump_applications),
    })
}

fn dump_map(map: &ThemeMap) -> serde_json::Value {
    match map {
        ThemeMap::Catppuccin(map) => serde_json::json!({
            "type": "map/catppuccin",
            "entries": map.entries.iter().map(|entry| serde_json::json!({
                "key": { "value": entry.key.value.as_str(), "span": span_json(entry.key.span) },
                "palette": reference_json(&entry.palette),
                "span": span_json(entry.span),
            })).collect::<Vec<_>>(),
            "span": span_json(map.span),
        }),
        ThemeMap::Eza(map) => serde_json::json!({
            "type": "map/eza",
            "categories": map.categories.iter().map(|category| serde_json::json!({
                "name": string_json(&category.name),
                "extensions": category.extensions.iter().map(|extension| serde_json::json!({
                    "value": extension.value.as_str(), "span": span_json(extension.span)
                })).collect::<Vec<_>>(),
                "span": span_json(category.span),
            })).collect::<Vec<_>>(),
            "span": span_json(map.span),
        }),
        ThemeMap::Gtk(map) => serde_json::json!({
            "type": "map/gtk",
            "entries": map.entries.iter().map(|entry| serde_json::json!({
                "key": string_json(&entry.key), "role": reference_json(&entry.role), "span": span_json(entry.span)
            })).collect::<Vec<_>>(),
            "span": span_json(map.span),
        }),
        ThemeMap::Kde(map) => serde_json::json!({
            "type": "map/kde",
            "groups": map.groups.iter().map(|entry| serde_json::json!({
                "key": string_json(&entry.key),
                "roles": entry.roles.iter().map(reference_json).collect::<Vec<_>>(),
                "span": span_json(entry.span),
            })).collect::<Vec<_>>(),
            "foregrounds": dump_kde_roles(&map.foregrounds),
            "selection-foregrounds": dump_kde_roles(&map.selection_foregrounds),
            "span": span_json(map.span),
        }),
        ThemeMap::Obsidian(map) => serde_json::json!({
            "type": "map/obsidian",
            "source": reference_json(&map.source),
            "variables": map.variables.iter().map(|variable| serde_json::json!({
                "key": string_json(&variable.key),
                "value": dump_obsidian_value(&variable.value),
                "span": span_json(variable.span),
            })).collect::<Vec<_>>(),
            "span": span_json(map.span),
        }),
    }
}

fn dump_kde_roles(entries: &[KdeRoleEntry]) -> Vec<serde_json::Value> {
    entries
        .iter()
        .map(|entry| serde_json::json!({
            "key": string_json(&entry.key), "role": reference_json(&entry.role), "span": span_json(entry.span)
        }))
        .collect()
}

fn dump_obsidian_value(value: &ObsidianValue) -> serde_json::Value {
    match value {
        ObsidianValue::Palette(reference) => {
            serde_json::json!({ "shape": "palette", "reference": reference_json(reference) })
        }
        ObsidianValue::Rgb(reference) => {
            serde_json::json!({ "shape": "rgb", "reference": reference_json(reference) })
        }
        ObsidianValue::Color { color, alpha } => serde_json::json!({
            "shape": "color", "reference": reference_json(color), "alpha": decimal_json(alpha)
        }),
        ObsidianValue::Derived(derived) => serde_json::json!({
            "shape": "derived",
            "value": match derived.value {
                ObsidianDerived::AccentH => "accent_h",
                ObsidianDerived::AccentS => "accent_s",
                ObsidianDerived::AccentL => "accent_l",
                ObsidianDerived::AccentHsl => "accent_hsl",
            },
            "span": span_json(derived.span),
        }),
        ObsidianValue::Literal(literal) => {
            serde_json::json!({ "shape": "literal", "literal": string_json(literal) })
        }
    }
}
