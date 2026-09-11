use super::super::{
    Result,
    color::Color,
    model::{Theme, table, text},
    render::lines,
};
use std::collections::BTreeMap;
pub fn colors(t: &Theme) -> Result<BTreeMap<String, Color>> {
    table(&t.map("tmux")?["colors"])?
        .iter()
        .map(|(k, v)| Ok((k.clone(), t.color(text(v))?)))
        .collect()
}
pub fn indexed(c: Color, bg: Color) -> Result<usize> {
    indexed_many(c, &[bg])
}
pub fn indexed_many(c: Color, backgrounds: &[Color]) -> Result<usize> {
    let mut best = None;
    for i in 16..256 {
        let rgb = if i >= 232 {
            [8 + 10 * (i - 232); 3]
        } else {
            let ramp = [0, 95, 135, 175, 215, 255];
            let n = i - 16;
            [ramp[n / 36], ramp[n / 6 % 6], ramp[n % 6]]
        };
        let color = Color(rgb.map(|x| x as u8));
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
        .ok_or_else(|| "no readable indexed tmux color".into())
}
pub fn pairs(t: &Theme) -> Result<Vec<(String, Color, Color, f64)>> {
    let map = t.map("tmux")?;
    let colors = colors(t)?;
    let mut rows = Vec::new();
    let mut push = |state: String, fg: &str, bg: &str, floor: f64| -> Result<()> {
        rows.push((
            state,
            *colors
                .get(fg)
                .ok_or_else(|| format!("unknown tmux color: {fg}"))?,
            *colors
                .get(bg)
                .ok_or_else(|| format!("unknown tmux color: {bg}"))?,
            floor,
        ));
        Ok(())
    };
    for (state, style) in table(&map["styles"])? {
        push(
            state.clone(),
            text(&style["fg"]),
            text(&style["bg"]),
            style["floor"].as_f64().unwrap_or(4.5),
        )?;
    }
    for (state, role) in table(&map["fzf"])? {
        if matches!(state.as_str(), "bg" | "bg+" | "preview-bg" | "gutter") {
            continue;
        }
        push(
            format!("fzf.{state}"),
            text(role),
            if state.ends_with('+') {
                "active_bg"
            } else {
                "bg"
            },
            if state == "border" { 3. } else { 4.5 },
        )?;
    }
    for (state, fg, bg) in [
        ("status.active", "active_fg", "active_bg"),
        ("status.muted", "muted", "bg"),
        ("status.success", "success", "bg"),
        ("status.warning", "warning", "bg"),
        ("status.error", "error", "bg"),
        ("pane.label", "primary", "bg"),
        ("pane.number", "muted", "bg"),
    ] {
        push(state.into(), fg, bg, 4.5)?;
    }
    Ok(rows)
}
pub fn render(t: &Theme) -> Result<String> {
    let mapping = t.map("tmux")?;
    let colors = colors(t)?;
    let color = |name: &str| {
        colors
            .get(name)
            .copied()
            .ok_or_else(|| format!("unknown tmux color: {name}"))
    };
    let mut out = vec![
        format!("# {}", t.header()),
        format!("set -g @theme_name '{}'", t.profile),
    ];
    for name in table(&mapping["colors"])?.keys() {
        out.push(format!("set -g @theme_{name} '{}'", color(name)?));
    }
    let fzf = table(&mapping["fzf"])?
        .iter()
        .map(|(name, role)| Ok(format!("{name}:{}", color(text(role))?)))
        .collect::<Result<Vec<_>>>()?
        .join(",");
    out.extend([format!("set -g @theme_fzf_colors '{fzf}'"), String::new()]);
    for (style, role, attrs) in [
        ("hint", "primary", ",bold"),
        ("highlight", "fg", ""),
        ("backdrop", "muted", ""),
        ("selected-hint", "success", ",bold"),
        ("selected-highlight", "success", ""),
    ] {
        out.push(format!(
            "set -g @fingers-{style}-style 'fg=colour{}{attrs}'",
            indexed(color(role)?, color("bg")?)?
        ));
    }
    for (name, style) in table(&mapping["styles"])? {
        let mut value = format!(
            "fg={},bg={}",
            color(text(&style["fg"]))?,
            color(text(&style["bg"]))?
        );
        let attrs = text(&style["attrs"]);
        if !attrs.is_empty() {
            value.push(',');
            value.push_str(attrs);
        }
        out.push(format!("set -g {name} '{value}'"));
    }
    out.extend([
        String::new(),
        format!("set -g display-panes-colour '{}'", color("muted")?),
        format!("set -g display-panes-active-colour '{}'", color("primary")?),
        format!("set -g clock-mode-colour '{}'", color("primary")?),
        format!("set -ag message-style ',fill={}'", color("surface")?),
        format!(
            "set -ag message-command-style ',fill={}'",
            color("active_bg")?
        ),
    ]);
    Ok(lines(out))
}
