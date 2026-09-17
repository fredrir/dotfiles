use super::{
    Result,
    color::Color,
    model::{ANSI, Repository, Theme, table, text},
};
use serde_json::Value;
#[derive(Clone, Debug)]
pub struct Pair {
    pub area: String,
    pub state: String,
    pub fg: Color,
    pub bg: Color,
    pub floor: f64,
    pub enforced: bool,
}
impl Pair {
    pub fn ratio(&self) -> f64 {
        self.fg.contrast(self.bg)
    }
    pub fn passes(&self) -> bool {
        self.ratio() + 1e-9 >= self.floor
    }
}
fn pair(area: &str, state: &str, fg: Color, bg: Color, floor: f64) -> Pair {
    Pair {
        area: area.into(),
        state: state.into(),
        fg,
        bg,
        floor,
        enforced: true,
    }
}
fn named(t: &Theme, area: &str, state: &str, fg: &str, bg: &str, floor: f64) -> Result<Pair> {
    Ok(pair(area, state, t.color(fg)?, t.color(bg)?, floor))
}
pub fn pairs(t: &Theme) -> Result<Vec<Pair>> {
    let mut rows = Vec::new();
    for spec in t.data.contracts["semantic_pairs"]
        .as_array()
        .ok_or("missing semantic contract")?
    {
        rows.push(named(
            t,
            "semantic",
            text(&spec[0]),
            text(&spec[1]),
            text(&spec[2]),
            spec[3].as_f64().unwrap_or(4.5),
        )?);
    }
    for name in ["info", "success", "warning", "error"] {
        for (state, fg, bg) in [
            (
                format!("{name}.on_fill"),
                format!("on_{name}"),
                format!("{name}_fill"),
            ),
            (
                format!("{name}.text.canvas"),
                format!("{name}_text_on_canvas"),
                "canvas_bg".into(),
            ),
            (
                format!("{name}.text.panel"),
                format!("{name}_text_on_panel"),
                "panel_bg".into(),
            ),
        ] {
            rows.push(named(t, "semantic", &state, &fg, &bg, 4.5)?);
        }
    }
    for group in super::resolve::kde(t)?.iter() {
        for foreground in &group.foregrounds {
            for (label, bg) in ["normal", "alternate"].iter().zip(group.backgrounds) {
                rows.push(pair(
                    "kde",
                    &format!("{}.{}.{label}", group.name, foreground.key),
                    foreground.color,
                    bg,
                    foreground.floor,
                ));
            }
        }
        for (label, bg) in ["normal", "alternate"].iter().zip(group.backgrounds) {
            rows.push(pair(
                "kde",
                &format!("{}.decoration.{label}", group.name),
                group.decoration,
                bg,
                3.,
            ));
        }
    }
    let mapping = &t.map("gtk")?["colors"];
    for spec in t.data.contracts["gtk_pairs"]
        .as_array()
        .ok_or("missing GTK contract")?
    {
        rows.push(pair(
            "gtk",
            text(&spec[0]),
            t.mapped("kde", text(&mapping[text(&spec[1])]))?,
            t.mapped("kde", text(&mapping[text(&spec[2])]))?,
            spec[3].as_f64().unwrap_or(4.5),
        ));
    }
    for spec in t.data.contracts["obsidian_pairs"]
        .as_array()
        .ok_or("missing Obsidian contract")?
    {
        rows.push(named(
            t,
            "obsidian",
            text(&spec[0]),
            text(&spec[1]),
            text(&spec[2]),
            spec[3].as_f64().unwrap_or(4.5),
        )?);
    }
    for name in ["info", "success", "warning", "error"] {
        rows.push(named(
            t,
            "obsidian",
            &format!("{name}.text"),
            &format!("{name}_text_on_canvas"),
            "canvas_bg",
            4.5,
        )?);
    }
    for name in ANSI {
        rows.push(named(
            t,
            "obsidian",
            &format!("code.{name}"),
            &format!("ansi_{name}_text_on_panel"),
            "panel_bg",
            4.5,
        )?);
    }
    rows.extend(yazi_pairs(t)?);
    let palette = super::emitters::ui::document(t)?;
    for (state, _) in super::emitters::ui::SEMANTICS {
        let backgrounds: &[&str] = match *state {
            "background" | "panel" | "surface" => continue,
            "selection_background" => &["background"],
            "panel_foreground" => &["panel"],
            "surface_foreground" => &["surface"],
            "selection_foreground" => &["selection_background"],
            _ => &["background", "panel", "surface"],
        };
        for background in backgrounds {
            let name = format!("{state}.{background}");
            let rgb = |name: &str| Color::parse(&palette.ui[name]);
            let floor = if *state == "selection_background" {
                3.0
            } else {
                4.5
            };
            rows.push(pair("ui", &name, rgb(state)?, rgb(background)?, floor));
            if *state != "selection_background" {
                let indexed = |name: &str| {
                    Color(
                        ui_theme::Color::Ansi(palette.ui_indexed[name])
                            .rgb()
                            .unwrap_or_default(),
                    )
                };
                rows.push(pair(
                    "ui256",
                    &name,
                    indexed(state),
                    indexed(background),
                    floor,
                ));
            }
        }
    }
    Ok(rows)
}
pub fn matrix(t: &Theme) -> Result<String> {
    let mut rows = pairs(t)?;
    for name in ANSI {
        for (prefix, expression) in [
            ("normal", name.to_string()),
            ("bright", format!("bright_{name}")),
        ] {
            let mut p = named(
                t,
                "ansi",
                &format!("{prefix}.{name}"),
                &expression,
                "canvas_bg",
                4.5,
            )?;
            p.enforced = false;
            rows.push(p);
        }
    }
    let mut out = format!(
        "# Contrast matrix: {}\n\n`{}`.\nRequired text pairs target 4.5:1; graphical and inactive pairs target 3:1.\nRaw ANSI rows are reported but are not enforced.\n\n| Area | State | Foreground | Background | Ratio | Floor | Result |\n|---|---|---:|---:|---:|---:|---|\n",
        t.name, t.profile
    );
    use std::fmt::Write;
    for p in rows {
        let result = if !p.enforced {
            "raw"
        } else if p.passes() {
            "pass"
        } else {
            "FAIL"
        };
        writeln!(
            out,
            "| {} | `{}` | `{}` | `{}` | {:.2}:1 | {:.1}:1 | {result} |",
            p.area,
            p.state,
            p.fg,
            p.bg,
            p.ratio(),
            p.floor
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(out)
}
fn walk<'a>(value: &'a Value, path: &str, found: &mut Vec<(String, &'a Value)>) {
    match value {
        Value::Object(table) => {
            if table.contains_key("fg") || table.contains_key("bg") {
                found.push((path.into(), value));
            }
            for (key, child) in table {
                walk(
                    child,
                    &if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    },
                    found,
                );
            }
        }
        Value::Array(values) => {
            for (i, child) in values.iter().enumerate() {
                walk(child, &format!("{path}.{i}"), found);
            }
        }
        _ => {}
    }
}
fn yazi_styles(t: &Theme) -> Result<Vec<(String, &Value)>> {
    let mut result = Vec::new();
    walk(t.map("yazi")?, "", &mut result);
    Ok(result)
}
fn yazi_schema(t: &Theme) -> Result<()> {
    let document = t.map("yazi")?;
    let mut problems = Vec::new();
    let schema = &t.data.contracts["yazi_schema"];
    for (section, body) in table(document)? {
        if schema[section].is_null() {
            problems.push(format!("unknown Yazi section: {section}"));
            continue;
        }
        for key in table(body)?.keys() {
            if !schema[section]
                .as_array()
                .is_some_and(|keys| keys.iter().any(|k| k.as_str() == Some(key)))
            {
                problems.push(format!("unknown Yazi key: {section}.{key}"));
            }
        }
    }
    for key in ["border", "chord", "action", "hovered"] {
        if document["help"].get(key).is_none() {
            problems.push(format!("Yazi [help] misses: {key}"));
        }
    }
    if document["which"].get("border").is_none() {
        problems.push("Yazi [which] misses: border".into());
    }
    for (state, style) in yazi_styles(t)? {
        if style["reversed"].as_bool() == Some(true) {
            problems.push(format!(
                "Yazi {state} uses reversed instead of an explicit pair"
            ));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n  "))
    }
}
fn yazi_pairs(t: &Theme) -> Result<Vec<Pair>> {
    let mut rows = Vec::new();
    let canvas = t.color("canvas_bg")?;
    for (state, style) in yazi_styles(t)? {
        let Some(fg_expr) = style["fg"].as_str().filter(|s| !s.is_empty()) else {
            continue;
        };
        let section = state.split('.').next().unwrap_or("");
        let context = t.color(if matches!(section, "status" | "which") {
            "panel_bg"
        } else {
            "canvas_bg"
        })?;
        let mut bg = match style["bg"].as_str() {
            None | Some("reset") => context,
            Some(e) => t.color(e)?,
        };
        let fg = t.color(fg_expr)?;
        let marker = state.starts_with("mgr.marker_") && fg == bg;
        let graphical = marker
            || [
                "border",
                "progress_",
                "separator",
                "indicator.parent",
                "indicator.preview",
            ]
            .iter()
            .any(|s| state.contains(s));
        if marker {
            bg = canvas;
        }
        let floor = if graphical || fg_expr.contains("disabled") {
            3.
        } else {
            4.5
        };
        rows.push(pair("yazi", &state, fg, bg, floor));
    }
    Ok(rows)
}
pub fn yazi_render(t: &Theme) -> Result<String> {
    yazi_schema(t)?;
    let re =
        regex::Regex::new(r#"(\b(?:fg|bg)\s*=\s*")([^"]+)(")"#).expect("valid Yazi expression");
    let template = &t.data.yazi;
    let mut rendered = String::new();
    let mut end = 0;
    for c in re.captures_iter(template) {
        let m = c.get(0).expect("full regex match");
        rendered.push_str(&template[end..m.start()]);
        let value = if &c[2] == "reset" {
            "reset".into()
        } else {
            t.color(&c[2])?.to_string()
        };
        rendered.push_str(&format!("{}{value}{}", &c[1], &c[3]));
        end = m.end();
    }
    rendered.push_str(&template[end..]);
    Ok(format!(
        "# Yazi theme contract: 26.8.15+\n\n# {}\n\n{rendered}",
        t.header()
    ))
}
fn expressions<'a>(value: &'a Value, found: &mut Vec<&'a str>) {
    match value {
        Value::Object(values) => {
            for child in values.values() {
                expressions(child, found);
            }
        }
        Value::String(s) => found.push(s),
        _ => {}
    }
}
pub fn validate(t: &Theme) -> Result<()> {
    let mut errors = Vec::new();
    let mut check = |r: Result<()>| {
        if let Err(e) = r {
            errors.push(e);
        }
    };
    for name in table(&t.data.contracts["semantics"])?.keys() {
        check(t.color(name).map(|_| ()));
    }
    for section in ["roles", "terminal", "eza", "kde", "konsole"] {
        let mut values = Vec::new();
        expressions(&t.data.roles[section], &mut values);
        for expression in values {
            check(t.color(expression).map(|_| ()));
        }
    }
    for name in ["general", "nerd"] {
        check(t.font(name).and_then(|s| {
            if s.contains(',') {
                Err(format!("font '{name}' must not contain a comma"))
            } else {
                Ok(())
            }
        }));
    }
    for size in ["terminal", "interface"] {
        check(t.size(size).map(|_| ()));
    }
    let bg = t.color("ui.background")?.luminance();
    let fg = t.color("ui.foreground")?.luminance();
    if (t.dark && bg >= fg) || (!t.dark && bg <= fg) {
        errors.push("profile dark flag disagrees with background/foreground lightness".into());
    }
    let mut check = |r: Result<()>| {
        if let Err(e) = r {
            errors.push(e);
        }
    };
    for value in table(&t.map("nvim")?["colors"])?.values() {
        check(t.color(text(value)).map(|_| ()));
    }
    for value in table(&t.map("gtk")?["colors"])?.values() {
        check(t.mapped("kde", text(value)).map(|_| ()));
    }
    let obsidian = t.map("obsidian")?;
    check(t.color(text(&obsidian["derived"]["source"])).map(|_| ()));
    for value in table(&obsidian["variables"])?.values() {
        let expr = value
            .as_str()
            .or_else(|| value["rgb"].as_str())
            .or_else(|| value["color"].as_str());
        if let Some(expr) = expr {
            check(t.resolve(expr).map(|_| ()));
        }
    }
    check(yazi_schema(t));
    for (_, style) in yazi_styles(t)? {
        for key in ["fg", "bg"] {
            if let Some(expr) = style[key]
                .as_str()
                .filter(|s| !s.is_empty() && *s != "reset")
            {
                check(t.color(expr).map(|_| ()));
            }
        }
    }
    match pairs(t) {
        Ok(pairs) => {
            for p in pairs {
                if !p.passes() {
                    errors.push(format!(
                        "{}.{}: {} on {} is {:.2}:1, under {:.1}:1",
                        p.area,
                        p.state,
                        p.fg,
                        p.bg,
                        p.ratio(),
                        p.floor
                    ));
                }
            }
        }
        Err(e) => errors.push(e),
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "profile '{}' is not usable:\n  {}",
            t.profile,
            errors.join("\n  ")
        ))
    }
}
pub fn all(repo: &Repository) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    let mut errors = Vec::new();
    if repo.themes.is_empty() {
        return Err("no profiles in theme/profiles".into());
    }
    for t in repo.themes.values() {
        if !names.insert(&t.name) {
            errors.push(format!("duplicate display name: {}", t.name));
        }
        if let Err(e) = validate(t) {
            errors.push(e);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}
