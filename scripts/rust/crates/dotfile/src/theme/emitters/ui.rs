use super::super::{
    Result,
    model::{Theme, table},
};
use std::collections::BTreeMap;
use std::rc::Rc;
use ui_theme::{Palette, PaletteDocument};

pub const SEMANTICS: &[(&str, &str)] = &[
    ("background", "canvas_bg"),
    ("panel", "panel_bg"),
    ("surface", "surface_fill"),
    ("foreground", "text_on_canvas"),
    ("panel_foreground", "text_on_panel"),
    ("surface_foreground", "text_on_surface"),
    ("muted", "muted_on_canvas"),
    ("accent", "primary_text_on_canvas"),
    ("border", "border_on_canvas"),
    ("focus", "focus_ring"),
    ("selection_background", "primary_fill_on_canvas"),
    ("selection_foreground", "on_primary_fill_on_canvas"),
    ("success", "success_text_on_canvas"),
    ("warning", "warning_text_on_canvas"),
    ("danger", "error_text_on_canvas"),
    ("info", "info_text_on_canvas"),
    ("diff_added", "success_text_on_canvas"),
    ("diff_removed", "error_text_on_canvas"),
    ("diff_context", "text_on_canvas"),
    ("diff_header", "primary_text_on_canvas"),
    ("ours", "ansi_magenta_text_on_canvas"),
    ("theirs", "ansi_cyan_text_on_canvas"),
    ("conflict", "warning_text_on_canvas"),
];

pub fn document(theme: &Theme) -> Result<Rc<PaletteDocument>> {
    if let Some(document) = theme.ui.borrow().as_ref() {
        return Ok(Rc::clone(document));
    }
    let mut colors = BTreeMap::new();
    for name in Theme::palette_names()
        .into_iter()
        .chain(["fg", "muted", "separator"].map(str::to_string))
    {
        colors.insert(name.clone(), theme.color(&name)?.to_string());
    }
    let roles = table(&theme.data.roles["roles"])?
        .keys()
        .map(|name| Ok((name.clone(), theme.role(name)?.to_string())))
        .collect::<Result<_>>()?;
    let neutral_surfaces = [
        theme.color("canvas_bg")?,
        theme.color("panel_bg")?,
        theme.color("surface_fill")?,
    ];
    let ui = SEMANTICS
        .iter()
        .map(|(name, expression)| {
            let color = match *name {
                "background"
                | "panel"
                | "surface"
                | "panel_foreground"
                | "surface_foreground"
                | "selection_background"
                | "selection_foreground" => theme.color(expression)?,
                _ => theme.readable_many(expression, &neutral_surfaces, 4.5)?,
            };
            Ok(((*name).into(), color.to_string()))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut ui_indexed = BTreeMap::new();
    for name in ["background", "panel", "surface", "selection_background"] {
        let color = ui_theme::Color::parse(&ui[name])?.at_depth(ui_theme::ColorDepth::Ansi256);
        if let ui_theme::Color::Ansi(index) = color {
            ui_indexed.insert(name.to_string(), index);
        }
    }
    for (name, _) in SEMANTICS {
        if ui_indexed.contains_key(*name) {
            continue;
        }
        let backgrounds: &[&str] = match *name {
            "panel_foreground" => &["panel"],
            "surface_foreground" => &["surface"],
            "selection_foreground" => &["selection_background"],
            _ => &["background", "panel", "surface"],
        };
        let backgrounds = backgrounds
            .iter()
            .map(|name| {
                ui_theme::Color::Ansi(ui_indexed[*name])
                    .rgb()
                    .map(super::super::color::Color)
                    .ok_or_else(|| "invalid indexed color".to_string())
            })
            .collect::<Result<Vec<_>>>()?;
        let foreground = super::super::color::Color::parse(&ui[*name])?;
        ui_indexed.insert(
            (*name).into(),
            indexed_many(foreground, &backgrounds)? as u8,
        );
    }
    let document = Rc::new(PaletteDocument {
        version: 1,
        profile: theme.profile.clone(),
        dark: theme.dark,
        colors,
        roles,
        ui,
        ui_indexed,
    });
    theme.ui.replace(Some(Rc::clone(&document)));
    Ok(document)
}

pub fn palette(theme: &Theme) -> Result<Palette> {
    Palette::from_document(document(theme)?.as_ref().clone())
}

pub fn render(theme: &Theme) -> Result<String> {
    let mut output = serde_json::to_string_pretty(document(theme)?.as_ref())
        .map_err(|error| error.to_string())?;
    output.push('\n');
    Ok(output)
}

fn indexed_many(
    c: super::super::color::Color,
    backgrounds: &[super::super::color::Color],
) -> Result<usize> {
    let mut best = None;
    for i in 16..256 {
        let rgb = if i >= 232 {
            [8 + 10 * (i - 232); 3]
        } else {
            let ramp = [0, 95, 135, 175, 215, 255];
            let n = i - 16;
            [ramp[n / 36], ramp[n / 6 % 6], ramp[n % 6]]
        };
        let color = super::super::color::Color(rgb.map(|x| x as u8));
        if backgrounds
            .iter()
            .all(|background| color.contrast(*background) >= 4.5)
        {
            let distance = rgb
                .iter()
                .zip(c.0)
                .map(|(a, b)| (*a as i32 - i32::from(b)).pow(2))
                .sum::<i32>();
            let candidate = (distance, i);
            if best.is_none_or(|old| candidate < old) {
                best = Some(candidate);
            }
        }
    }
    best.map(|(_, i)| i)
        .ok_or_else(|| "no readable indexed color".into())
}
