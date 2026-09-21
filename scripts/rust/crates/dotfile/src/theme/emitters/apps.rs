use super::{
    super::{
        Result,
        color::Color,
        model::{Repository, Theme, table, text},
        render::{self, lines},
    },
    Kind,
};
use regex::Regex;
use std::collections::HashMap;
fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid static expression")
}
const SECTIONS: [&str; 4] = [
    "section_system",
    "section_hardware",
    "section_desktop",
    "section_network",
];
pub fn render(repo: &Repository, t: &Theme, kind: &Kind, previous: &str) -> Result<String> {
    match kind {
        Kind::FastfetchConfig => {
            let mut out = Vec::new();
            for role in SECTIONS {
                let c = t.role(role)?;
                out.push(format!(
                    "\"\\u001b[1;{}m\", // {} {c}",
                    c.ansi(),
                    text(&t.data.roles["roles"][role])
                ));
            }
            let c = t.role("separator")?;
            out.push(format!(
                "\"\\u001b[{}m\" // {} {c}",
                c.ansi(),
                text(&t.data.roles["roles"]["separator"])
            ));
            let changed = render::between(previous, "constants", &out)?;
            let re = regex(r"#[0-9a-fA-F]{6}");
            Ok(changed
                .split('\n')
                .map(|line| {
                    if line.contains("theme:separator") {
                        re.replace_all(line, c.to_string()).into_owned()
                    } else {
                        line.into()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        Kind::FastfetchLogo => logo(t, previous),
        Kind::Starship => starship(t, previous),
        Kind::Obsidian => obsidian(t, previous),
        Kind::Nvim => nvim(t, previous),
        Kind::Gtk => gtk(t, previous),
        Kind::GtkSettings => {
            let s = render::ini(
                previous,
                "Settings",
                "gtk-font-name",
                &format!("{},  {}", t.font("general")?, t.size("interface")?),
            )?;
            let s = render::ini(
                &s,
                "Settings",
                "gtk-application-prefer-dark-theme",
                if t.dark { "true" } else { "false" },
            )?;
            render::ini(&s, "Settings", "gtk-icon-theme-name", t.icons())
        }
        Kind::Quicklaunch => {
            let mut out = vec![format!("# {}", t.header())];
            for (key, role) in [
                ("accent", "accent"),
                ("background", "view_bg"),
                ("text", "foreground"),
                ("muted", "inactive"),
                ("selection", "selection_bg"),
            ] {
                out.push(format!("{key:10} = \"{}\"", t.app("kde", role)?));
            }
            render::between(previous, "quicklaunch", &out)
        }
        Kind::Panel => remap(repo, t, previous),
        Kind::Plasma => plasma(t, previous),
        Kind::Desktop => desktop(repo, t, previous),
        _ => Err("unsupported emitter".into()),
    }
}
fn logo(t: &Theme, previous: &str) -> Result<String> {
    let stops = SECTIONS
        .iter()
        .map(|r| t.role(r).map(|c| c.0))
        .collect::<Result<Vec<_>>>()?;
    let trailing = previous.ends_with('\n');
    let raw = previous
        .strip_suffix('\n')
        .unwrap_or(previous)
        .split('\n')
        .collect::<Vec<_>>();
    let re = regex("\x1b\\[[0-9;]*m");
    let mut out = Vec::new();
    for (i, line) in raw.iter().enumerate() {
        let pos = if raw.len() > 1 {
            i as f64 / (raw.len() - 1) as f64 * 3.
        } else {
            0.
        };
        let seg = (pos as usize).min(2);
        let ratio = pos - seg as f64;
        let c = Color(std::array::from_fn(|j| {
            (f64::from(stops[seg][j])
                + (f64::from(stops[seg + 1][j]) - f64::from(stops[seg][j])) * ratio)
                .round_ties_even() as u8
        }));
        out.push(format!("\x1b[1;{}m{}", c.ansi(), re.replace_all(line, "")));
    }
    Ok(out.join("\n") + "\x1b[0m" + if trailing { "\n" } else { "" })
}
fn starship(t: &Theme, previous: &str) -> Result<String> {
    let names = Theme::palette_names();
    let width = names.iter().map(String::len).max().unwrap_or(0);
    let mut out = vec![format!("# {}", t.header()), "[palettes.theme]".into()];
    for name in names {
        out.push(format!("{name:width$} = '{}'", t.color(&name)?));
    }
    out.push(String::new());
    let roles = [
        "prompt_python",
        "prompt_git",
        "prompt_dir",
        "prompt_duration",
        "prompt_char",
    ];
    let width = roles.iter().map(|s| s.len()).max().unwrap_or(0);
    for role in roles {
        out.push(format!("{role:width$} = '{}'", t.role(role)?));
    }
    render::between(previous, "palette", &out)
}
pub fn zsh(t: &Theme) -> Result<String> {
    let mut out = vec![
        format!("# {}", t.header()),
        "export THEME_RESET=$'\\e[0m'".into(),
    ];
    for (env, role) in [
        ("SUDO", "sudo"),
        ("GIT", "prompt_git"),
        ("DIR", "prompt_dir"),
        ("CHAR", "prompt_char"),
    ] {
        out.push(format!(
            "export THEME_{env}=$'\\e[{}m'",
            t.role(role)?.ansi()
        ));
    }
    for (env, role) in [
        ("SELECTION_FG", "selection_foreground"),
        ("SELECTION_BG", "selection_background"),
    ] {
        out.push(format!(
            "export THEME_{env}='{}'",
            t.app("terminal", role)?
        ));
    }
    if let Some(eza) = t.data.roles["eza"].as_object().filter(|m| !m.is_empty()) {
        let mut parts = vec!["reset".into()];
        for kind in ["fi", "di", "ex", "ln", "pi", "so", "bd", "cd"] {
            if let Some(v) = eza.get(kind) {
                parts.push(format!("{kind}={}", t.color(text(v))?.ansi()));
            }
        }
        if let Some(categories) = eza.get("categories").and_then(|v| v.as_object()) {
            for (category, color) in categories {
                for extension in text(&t.map("eza")?["categories"][category]).split_whitespace() {
                    parts.push(format!("*.{extension}={}", t.color(text(color))?.ansi()));
                }
            }
        }
        for (key, color) in eza {
            if key.starts_with('*') {
                parts.push(format!("{key}={}", t.color(text(color))?.ansi()));
            }
        }
        out.push("unset LS_COLORS".into());
        out.push(format!("export EZA_COLORS=\"{}\"", parts.join(":")));
    }
    Ok(lines(out))
}
fn nvim(t: &Theme, previous: &str) -> Result<String> {
    let spec = t.map("nvim")?;
    let flavour = text(&spec["flavour"][if t.dark { "dark" } else { "light" }]);
    let unit = if previous.contains("\n\t") {
        "\t"
    } else {
        "  "
    };
    let mut body = vec![
        format!("flavour = \"{flavour}\","),
        "color_overrides = {".into(),
        format!("{unit}all = {{"),
    ];
    for (name, value) in table(&spec["colors"])? {
        body.push(format!(
            "{unit}{unit}{name} = \"{}\",",
            t.color(text(value))?
        ));
    }
    body.extend([format!("{unit}}},"), "},".into()]);
    let mut old = previous.split('\n').map(str::to_string).collect::<Vec<_>>();
    let first = old
        .iter()
        .position(|line| line.trim_start().starts_with("flavour ="))
        .ok_or("'flavour' setting not found")?;
    let table = (first + 1..old.len())
        .find(|i| !old[*i].trim().is_empty())
        .ok_or("'color_overrides' must follow 'flavour'")?;
    if !old[table].trim_start().starts_with("color_overrides = {") {
        return Err("'color_overrides' must follow 'flavour'".into());
    }
    let mut depth = 0isize;
    let last = (table..old.len())
        .find(|i| {
            depth += old[*i].matches('{').count() as isize - old[*i].matches('}').count() as isize;
            depth == 0
        })
        .ok_or("'color_overrides' table is not closed")?;
    let indent = &old[first][..old[first].len() - old[first].trim_start().len()];
    let replacement = body
        .iter()
        .map(|line| format!("{indent}{line}"))
        .collect::<Vec<_>>();
    old.splice(first..=last, replacement);
    Ok(old.join("\n"))
}
fn obsidian(t: &Theme, previous: &str) -> Result<String> {
    let spec = t.map("obsidian")?;
    let [r, g, b] = t
        .color(text(&spec["derived"]["source"]))?
        .0
        .map(|v| f64::from(v) / 255.);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.;
    let (h, s) = if min == max {
        (0., 0.)
    } else {
        let s = if l <= 0.5 {
            (max - min) / (max + min)
        } else {
            (max - min) / (2. - max - min)
        };
        let rc = (max - r) / (max - min);
        let gc = (max - g) / (max - min);
        let bc = (max - b) / (max - min);
        let hue = if r == max {
            bc - gc
        } else if g == max {
            2. + rc - bc
        } else {
            4. + gc - rc
        };
        ((hue / 6.).rem_euclid(1.), s)
    };
    let h = (h * 360.).round_ties_even();
    let s = (s * 100.).round_ties_even();
    let l = (l * 100.).round_ties_even();
    let derived = HashMap::from([
        ("accent_h", format!("{h}")),
        ("accent_s", format!("{s}%")),
        ("accent_l", format!("{l}%")),
        ("accent_hsl", format!("{h}, {s}%, {l}%")),
    ]);
    let mut out = vec![format!(
        "color-scheme: {};",
        if t.dark { "dark" } else { "light" }
    )];
    for (name, value) in table(&spec["variables"])? {
        let rendered = if let Some(expr) = value.as_str() {
            t.css(expr)?
        } else if let Some(v) = value.get("literal") {
            text(v).into()
        } else if let Some(v) = value.get("derived") {
            derived
                .get(text(v))
                .cloned()
                .ok_or("unknown derived variable")?
        } else if let Some(v) = value.get("rgb") {
            t.color(text(v))?
                .0
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            let c = t
                .color(text(&value["color"]))?
                .0
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("rgba({c}, {})", value["alpha"])
        };
        out.push(format!("{name}: {rendered};"));
    }
    if t.uses_fonts("obsidian")? {
        let general = render::lua(t.font("general")?);
        let nerd = render::lua(t.font("nerd")?);
        out.extend([
            format!("--font-interface-theme: {general}, sans-serif;"),
            format!("--font-text-theme: {general}, sans-serif;"),
            format!("--font-monospace-theme: {nerd}, ui-monospace, monospace;"),
        ]);
    }
    render::between(previous, "variables", &out)
}
fn gtk(t: &Theme, previous: &str) -> Result<String> {
    let mapping = &t.map("gtk")?["colors"];
    let re = regex(r"^(@define-color\s+)(\S+)(\s+)(#[0-9a-fA-F]{6,8})(;.*)$");
    previous
        .split('\n')
        .map(|line| {
            if let Some(c) = re.captures(line) {
                let var = &c[2];
                let base = var.strip_suffix("_breeze").unwrap_or(var);
                if let Some(role) = mapping.get(base) {
                    return Ok(format!(
                        "{}{}{}{}{}",
                        &c[1],
                        var,
                        &c[3],
                        t.mapped("kde", text(role))?,
                        &c[5]
                    ));
                }
                eprintln!("dotfile theme: unmapped GTK color '{var}'");
            }
            Ok(line.to_string())
        })
        .collect::<Result<Vec<_>>>()
        .map(|lines| lines.join("\n"))
}
fn hex_names(repo: &Repository, t: &Theme) -> Result<HashMap<Color, String>> {
    let mut map = HashMap::new();
    let ordered = std::iter::once(t).chain(repo.themes.values().filter(|x| x.profile != t.profile));
    for theme in ordered {
        for name in Theme::palette_names() {
            map.entry(theme.color(&name)?).or_insert(name);
        }
    }
    for (hex, name) in table(&t.map("catppuccin")?["colors"])? {
        map.entry(Color::parse(hex)?).or_insert(text(name).into());
    }
    Ok(map)
}
fn remap_with(t: &Theme, previous: &str, map: &HashMap<Color, String>) -> Result<String> {
    let re = regex(r"#([0-9a-fA-F]{8}|[0-9a-fA-F]{6})");
    let mut result = String::with_capacity(previous.len());
    let mut end = 0;
    for c in re.captures_iter(previous) {
        let m = c.get(0).expect("full regex match");
        result.push_str(&previous[end..m.start()]);
        let token = &c[1];
        if token.len() == 6 {
            if let Some(name) = map.get(&Color::parse(token)?) {
                result.push_str(&t.color(name)?.to_string());
            } else {
                result.push_str(m.as_str());
            }
        } else {
            result.push_str(m.as_str());
        }
        end = m.end();
    }
    result.push_str(&previous[end..]);
    Ok(result)
}
fn remap(repo: &Repository, t: &Theme, previous: &str) -> Result<String> {
    remap_with(t, previous, &hex_names(repo, t)?)
}
fn desktop(repo: &Repository, t: &Theme, previous: &str) -> Result<String> {
    let map = hex_names(repo, t)?;
    let mut rgb = HashMap::new();
    for (c, name) in &map {
        rgb.insert(c.csv(), t.color(name)?.csv());
    }
    let re = regex(r"^([^=\[]+)=(\d{1,3},\d{1,3},\d{1,3})$");
    Ok(remap_with(t, previous, &map)?
        .split('\n')
        .map(|line| {
            if let Some(c) = re.captures(line)
                && let Some(value) = rgb.get(&c[2])
            {
                return format!("{}={value}", &c[1]);
            }
            line.into()
        })
        .collect::<Vec<_>>()
        .join("\n"))
}
fn plasma(t: &Theme, previous: &str) -> Result<String> {
    let mut updated = previous.to_string();
    for group in super::super::resolve::kde(t)?.iter() {
        let mut body = vec![
            format!("BackgroundAlternate={}", group.backgrounds[1].csv()),
            format!("BackgroundNormal={}", group.backgrounds[0].csv()),
            format!("DecorationFocus={}", group.decoration.csv()),
            format!("DecorationHover={}", group.decoration.csv()),
        ];
        for foreground in &group.foregrounds {
            body.push(format!("{}={}", foreground.key, foreground.color.csv()));
        }
        updated = render::section(&updated, &group.name, &body)?;
    }
    let mut body = Vec::new();
    for (key, role) in [
        ("activeBackground", "wm_active_bg"),
        ("activeBlend", "wm_active_blend"),
        ("activeForeground", "wm_active_fg"),
        ("inactiveBackground", "wm_inactive_bg"),
        ("inactiveBlend", "wm_inactive_blend"),
        ("inactiveForeground", "wm_inactive_fg"),
    ] {
        body.push(format!("{key}={}", t.app("kde", role)?.csv()));
    }
    updated = render::section(&updated, "WM", &body)?;
    let accent = t.app("kde", "accent")?.csv();
    updated = render::ini(&updated, "General", "AccentColor", &accent)?;
    render::ini(&updated, "General", "LastUsedCustomAccentColor", &accent)
}
