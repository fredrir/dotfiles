use std::collections::HashMap;

use dotfile_source::ByteRange;
use dotfile_syntax::{Block, File};

use crate::model::{
    Appearance, ApplicationMap, ApplicationSetting, ApplicationState, EzaPattern, EzaRoles,
    FontBinding, FontMap, HexColor, NvimSettings, Palette, PaletteBinding, RoleBinding, RoleMap,
    Spanned, TerminalRoles, ThemeFonts, ThemeIdentity, ThemeOverrides, ThemeProfile, ThemeRoles,
    ThemeSizes,
};

use super::common::{Context, DecimalConstraint, name_spanned};

pub(super) fn lower_roles(context: &mut Context<'_>, file: File<'_>) -> Option<ThemeRoles> {
    let span = file.range();
    let mut seen = HashMap::new();
    let mut roles = None;
    let mut terminal = None;
    let mut eza = None;
    let mut kde = None;
    let mut konsole = None;

    for entry in file.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = named.name() else {
            continue;
        };
        if !matches!(name, "roles" | "terminal" | "eza" | "kde" | "konsole") {
            context.schema(
                named.range(),
                format!("unknown root field `{name}` in theme roles"),
            );
            continue;
        }
        if !context.track_unique(
            &mut seen,
            name,
            named.name_range().unwrap_or_else(|| named.range()),
            "theme roles block",
        ) {
            continue;
        }
        let Some(block) = context.require_block(named, name) else {
            continue;
        };
        match name {
            "roles" => roles = Some(lower_role_map(context, block)),
            "terminal" => terminal = Some(lower_terminal(context, block)),
            "eza" => eza = Some(lower_eza_roles(context, block)),
            "kde" => kde = Some(lower_role_map(context, block)),
            "konsole" => konsole = Some(lower_role_map(context, block)),
            _ => unreachable!(),
        }
    }

    Some(ThemeRoles {
        roles,
        terminal,
        eza,
        kde,
        konsole,
        span,
    })
}

pub(super) fn lower_fonts(context: &mut Context<'_>, file: File<'_>) -> Option<ThemeFonts> {
    let span = file.range();
    let mut seen = HashMap::new();
    let mut fonts = None;
    let mut sizes = None;
    let mut applications = None;

    for entry in file.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = named.name() else {
            continue;
        };
        if !matches!(name, "fonts" | "sizes" | "applications") {
            context.schema(
                named.range(),
                format!("unknown root field `{name}` in theme fonts"),
            );
            continue;
        }
        if !context.track_unique(
            &mut seen,
            name,
            named.name_range().unwrap_or_else(|| named.range()),
            "theme fonts block",
        ) {
            continue;
        }
        let Some(block) = context.require_block(named, name) else {
            continue;
        };
        match name {
            "fonts" => fonts = Some(lower_font_map(context, block, true)),
            "sizes" => sizes = Some(lower_sizes(context, block, true)),
            "applications" => applications = Some(lower_applications(context, block)),
            _ => unreachable!(),
        }
    }

    if !seen.contains_key("fonts") {
        context.missing(span, "fonts");
    }
    if !seen.contains_key("sizes") {
        context.missing(span, "sizes");
    }
    if !seen.contains_key("applications") {
        context.missing(span, "applications");
    }
    Some(ThemeFonts {
        fonts: fonts?,
        sizes: sizes?,
        applications: applications?,
        span,
    })
}

pub(super) fn lower_profile(
    context: &mut Context<'_>,
    file: File<'_>,
    identity: ThemeIdentity,
) -> Option<ThemeProfile> {
    let span = file.range();
    let mut seen = HashMap::new();
    let mut display_name = None;
    let mut appearance = None;
    let mut icons = None;
    let mut nvim = None;
    let mut palette = None;
    let mut overrides = ThemeOverrides::default();

    for entry in file.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = named.name() else {
            continue;
        };
        if !matches!(
            name,
            "display-name"
                | "appearance"
                | "icons"
                | "nvim"
                | "palette"
                | "roles"
                | "terminal"
                | "eza"
                | "kde"
                | "konsole"
                | "fonts"
                | "sizes"
                | "applications"
        ) {
            context.schema(
                named.range(),
                format!("unknown root field `{name}` in theme profile"),
            );
            continue;
        }
        if !context.track_unique(
            &mut seen,
            name,
            named.name_range().unwrap_or_else(|| named.range()),
            "theme profile field",
        ) {
            continue;
        }

        match name {
            "display-name" => {
                display_name = context
                    .require_value(named, name)
                    .and_then(|value| context.nonempty_string(value, name));
            }
            "appearance" => {
                appearance = context
                    .require_value(named, name)
                    .and_then(|value| lower_appearance(context, value));
            }
            "icons" => {
                icons = context
                    .require_value(named, name)
                    .and_then(|value| context.nonempty_string(value, name));
            }
            "nvim" => {
                nvim = context
                    .require_block(named, name)
                    .and_then(|block| lower_nvim(context, block));
            }
            "palette" => {
                palette = context
                    .require_block(named, name)
                    .map(|block| lower_palette(context, block));
            }
            "roles" => {
                overrides.roles = context
                    .require_block(named, name)
                    .map(|block| lower_role_map(context, block));
            }
            "terminal" => {
                overrides.terminal = context
                    .require_block(named, name)
                    .map(|block| lower_terminal(context, block));
            }
            "eza" => {
                overrides.eza = context
                    .require_block(named, name)
                    .map(|block| lower_eza_roles(context, block));
            }
            "kde" => {
                overrides.kde = context
                    .require_block(named, name)
                    .map(|block| lower_role_map(context, block));
            }
            "konsole" => {
                overrides.konsole = context
                    .require_block(named, name)
                    .map(|block| lower_role_map(context, block));
            }
            "fonts" => {
                overrides.fonts = context
                    .require_block(named, name)
                    .map(|block| lower_font_map(context, block, false));
            }
            "sizes" => {
                overrides.sizes = context
                    .require_block(named, name)
                    .map(|block| lower_sizes(context, block, false));
            }
            "applications" => {
                overrides.applications = context
                    .require_block(named, name)
                    .map(|block| lower_applications(context, block));
            }
            _ => unreachable!(),
        }
    }

    for name in ["display-name", "appearance", "icons", "nvim", "palette"] {
        if !seen.contains_key(name) {
            context.missing(span, name);
        }
    }

    Some(ThemeProfile {
        identity,
        display_name: display_name?,
        appearance: appearance?,
        icons: icons?,
        nvim: nvim?,
        palette: palette?,
        overrides,
        span,
    })
}

fn lower_role_map(context: &mut Context<'_>, block: Block<'_>) -> RoleMap {
    let span = block.range();
    let mut entries = Vec::new();
    let mut seen = HashMap::new();
    for entry in block.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = name_spanned(named) else {
            continue;
        };
        if !context.track_unique(&mut seen, &name.value, name.span, "role-map key") {
            continue;
        }
        let Some(value) = context.require_value(named, &name.value) else {
            continue;
        };
        let Some(palette) = context.reference(value, &name.value) else {
            continue;
        };
        entries.push(RoleBinding {
            name,
            palette,
            span: named.range(),
        });
    }
    RoleMap { entries, span }
}

fn lower_terminal(context: &mut Context<'_>, block: Block<'_>) -> TerminalRoles {
    let span = block.range();
    let mut direct = Vec::new();
    let mut direct_seen = HashMap::new();
    let mut structural_seen = HashMap::new();
    let mut ansi = None;
    let mut tabs = None;
    for entry in block.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = name_spanned(named) else {
            continue;
        };
        if matches!(name.value.as_str(), "ansi" | "tabs") {
            if !context.track_unique(
                &mut structural_seen,
                &name.value,
                name.span,
                "terminal structural block",
            ) {
                continue;
            }
            let Some(child) = context.require_block(named, &name.value) else {
                continue;
            };
            let roles = lower_role_map(context, child);
            match name.value.as_str() {
                "ansi" => ansi = Some(roles),
                "tabs" => tabs = Some(roles),
                _ => unreachable!(),
            }
            continue;
        }
        if !context.track_unique(
            &mut direct_seen,
            &name.value,
            name.span,
            "terminal role key",
        ) {
            continue;
        }
        let Some(value) = context.require_value(named, &name.value) else {
            continue;
        };
        let Some(palette) = context.reference(value, &name.value) else {
            continue;
        };
        direct.push(RoleBinding {
            name,
            palette,
            span: named.range(),
        });
    }
    TerminalRoles {
        direct,
        ansi,
        tabs,
        span,
    }
}

fn lower_eza_roles(context: &mut Context<'_>, block: Block<'_>) -> EzaRoles {
    let span = block.range();
    let mut direct = Vec::new();
    let mut direct_seen = HashMap::new();
    let mut categories = None;
    let mut categories_spans = Vec::new();
    let mut patterns = Vec::new();
    let mut pattern_keys = HashMap::new();

    for entry in block.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = name_spanned(named) else {
            continue;
        };
        match name.value.as_str() {
            "categories" => {
                if !categories_spans.is_empty() {
                    context.duplicate(
                        name.span,
                        &categories_spans,
                        "duplicate Eza `categories` block",
                    );
                    categories_spans.push(name.span);
                    continue;
                }
                categories_spans.push(name.span);
                categories = context
                    .require_block(named, "categories")
                    .map(|child| lower_role_map(context, child));
            }
            "pattern" => {
                let Some(child) = context.require_block(named, "pattern") else {
                    continue;
                };
                let Some(pattern) = lower_pattern(context, child, named.range(), &mut pattern_keys)
                else {
                    continue;
                };
                patterns.push(pattern);
            }
            _ => {
                if !context.track_unique(&mut direct_seen, &name.value, name.span, "Eza role key") {
                    continue;
                }
                let Some(value) = context.require_value(named, &name.value) else {
                    continue;
                };
                let Some(palette) = context.reference(value, &name.value) else {
                    continue;
                };
                direct.push(RoleBinding {
                    name,
                    palette,
                    span: named.range(),
                });
            }
        }
    }
    EzaRoles {
        direct,
        categories,
        patterns,
        span,
    }
}

fn lower_pattern(
    context: &mut Context<'_>,
    block: Block<'_>,
    record_span: ByteRange,
    pattern_keys: &mut HashMap<String, Vec<ByteRange>>,
) -> Option<EzaPattern> {
    let mut seen = HashMap::new();
    let mut key = None;
    let mut role = None;
    for entry in block.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = named.name() else {
            continue;
        };
        if !matches!(name, "key" | "role") {
            context.schema(named.range(), format!("unknown Eza pattern field `{name}`"));
            continue;
        }
        if !context.track_unique(
            &mut seen,
            name,
            named.name_range().unwrap_or_else(|| named.range()),
            "Eza pattern field",
        ) {
            continue;
        }
        let Some(value) = context.require_value(named, name) else {
            continue;
        };
        match name {
            "key" => key = context.literal_string(value, "pattern.key"),
            "role" => role = context.reference(value, "pattern.role"),
            _ => unreachable!(),
        }
    }
    if !seen.contains_key("key") {
        context.missing(block.range(), "pattern.key");
    }
    if !seen.contains_key("role") {
        context.missing(block.range(), "pattern.role");
    }
    let key = key?;
    if !context.track_unique(pattern_keys, &key.value, key.span, "Eza pattern key") {
        return None;
    }
    Some(EzaPattern {
        key,
        role: role?,
        span: record_span,
    })
}

fn lower_font_map(context: &mut Context<'_>, block: Block<'_>, required: bool) -> FontMap {
    let span = block.range();
    let mut entries = Vec::new();
    let mut seen = HashMap::new();
    for entry in block.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = name_spanned(named) else {
            continue;
        };
        if !context.track_unique(&mut seen, &name.value, name.span, "font key") {
            continue;
        }
        let Some(value) = context.require_value(named, &name.value) else {
            continue;
        };
        let Some(family) = context.nonempty_string(value, &name.value) else {
            continue;
        };
        if family.value.contains(',') {
            context.schema(
                family.span,
                format!("font family `{}` must not contain a comma", name.value),
            );
            continue;
        }
        entries.push(FontBinding {
            name,
            family,
            span: named.range(),
        });
    }
    if required {
        for name in ["general", "nerd"] {
            if !seen.contains_key(name) {
                context.missing(span, &format!("fonts.{name}"));
            }
        }
    }
    FontMap { entries, span }
}

fn lower_sizes(context: &mut Context<'_>, block: Block<'_>, required: bool) -> ThemeSizes {
    let span = block.range();
    let mut seen = HashMap::new();
    let mut terminal = None;
    let mut terminal_mac = None;
    let mut interface = None;
    for entry in block.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = named.name() else {
            continue;
        };
        if !matches!(name, "terminal" | "terminal_mac" | "interface") {
            context.schema(named.range(), format!("unknown theme size field `{name}`"));
            continue;
        }
        if !context.track_unique(
            &mut seen,
            name,
            named.name_range().unwrap_or_else(|| named.range()),
            "theme size field",
        ) {
            continue;
        }
        let Some(value) = context.require_value(named, name) else {
            continue;
        };
        let decimal = context.canonical_decimal(value, name, DecimalConstraint::Positive);
        match name {
            "terminal" => terminal = decimal,
            "terminal_mac" => terminal_mac = decimal,
            "interface" => interface = decimal,
            _ => unreachable!(),
        }
    }
    if required {
        for name in ["terminal", "terminal_mac", "interface"] {
            if !seen.contains_key(name) {
                context.missing(span, &format!("sizes.{name}"));
            }
        }
    }
    ThemeSizes {
        terminal,
        terminal_mac,
        interface,
        span,
    }
}

fn lower_applications(context: &mut Context<'_>, block: Block<'_>) -> ApplicationMap {
    let span = block.range();
    let mut entries = Vec::new();
    let mut seen = HashMap::new();
    for entry in block.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = name_spanned(named) else {
            continue;
        };
        if !context.track_unique(&mut seen, &name.value, name.span, "application key") {
            continue;
        }
        let Some(value) = context.require_value(named, &name.value) else {
            continue;
        };
        let Some(state) = context.literal_string(value, &name.value) else {
            continue;
        };
        let state_value = match state.value.as_str() {
            "enabled" => ApplicationState::Enabled,
            "disabled" => ApplicationState::Disabled,
            _ => {
                context.schema(
                    state.span,
                    format!(
                        "application `{}` must be `enabled` or `disabled`",
                        name.value
                    ),
                );
                continue;
            }
        };
        entries.push(ApplicationSetting {
            name,
            state: Spanned::new(state_value, state.span),
            span: named.range(),
        });
    }
    ApplicationMap { entries, span }
}

fn lower_appearance(
    context: &mut Context<'_>,
    value: dotfile_syntax::Value<'_>,
) -> Option<Spanned<Appearance>> {
    let appearance = context.literal_string(value, "appearance")?;
    let value = match appearance.value.as_str() {
        "dark" => Appearance::Dark,
        "light" => Appearance::Light,
        _ => {
            context.schema(appearance.span, "appearance must be `dark` or `light`");
            return None;
        }
    };
    Some(Spanned::new(value, appearance.span))
}

fn lower_nvim(context: &mut Context<'_>, block: Block<'_>) -> Option<NvimSettings> {
    let span = block.range();
    let mut seen = HashMap::new();
    let mut flavour = None;
    for entry in block.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = named.name() else {
            continue;
        };
        if name != "flavour" {
            context.schema(named.range(), format!("unknown nvim field `{name}`"));
            continue;
        }
        if !context.track_unique(
            &mut seen,
            name,
            named.name_range().unwrap_or_else(|| named.range()),
            "nvim field",
        ) {
            continue;
        }
        flavour = context
            .require_value(named, name)
            .and_then(|value| context.nonempty_string(value, "nvim.flavour"));
    }
    if !seen.contains_key("flavour") {
        context.missing(span, "nvim.flavour");
    }
    Some(NvimSettings {
        flavour: flavour?,
        span,
    })
}

fn lower_palette(context: &mut Context<'_>, block: Block<'_>) -> Palette {
    let span = block.range();
    let mut entries = Vec::new();
    let mut names = HashMap::new();
    let mut colors = HashMap::new();
    for entry in block.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = name_spanned(named) else {
            continue;
        };
        let unique_name = context.track_unique(&mut names, &name.value, name.span, "palette key");
        let Some(value) = context.require_value(named, &name.value) else {
            continue;
        };
        let Some(color) = context.literal_string(value, &name.value) else {
            continue;
        };
        let unique_color =
            context.track_unique(&mut colors, &color.value, color.span, "palette color value");
        if !is_hash_hex(&color.value) {
            context.schema(
                color.span,
                format!(
                    "palette `{}` must be lowercase `#[0-9a-f]{{6}}`",
                    name.value
                ),
            );
            continue;
        }
        if unique_name && unique_color {
            entries.push(PaletteBinding {
                name,
                color: Spanned::new(HexColor(color.value), color.span),
                span: named.range(),
            });
        }
    }
    Palette { entries, span }
}

fn is_hash_hex(text: &str) -> bool {
    text.len() == 7
        && text.starts_with('#')
        && text.as_bytes()[1..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}
