use super::super::{
    Result,
    model::{ANSI, Repository, Theme, UI, text},
    render::{lines, lua, lua_key},
};
pub fn render(repo: &Repository, t: &Theme, path: &str) -> Result<String> {
    let name = path
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim_end_matches(".lua");
    if path.contains("/colors/") && name != "profiles" {
        let scheme = repo.theme(name)?;
        let mut out = vec![
            format!("-- {}", scheme.header()),
            "".into(),
            "---@type ColorProfile".into(),
            "return {".into(),
            format!("  name = {},", lua(&scheme.name)),
            "  colors = {".into(),
        ];
        for key in UI {
            out.push(format!(
                "    {} = {},",
                lua_key(key),
                lua(text(&scheme.raw["ui"][key]))
            ));
        }
        for (key, group) in [("ansi", "normal"), ("brights", "bright")] {
            out.push(format!(
                "    {key} = {{ {} }},",
                ANSI.iter()
                    .map(|name| lua(text(&scheme.raw["ansi"][group][*name])))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out.extend(["  },".into(), "}".into()]);
        return Ok(lines(out));
    }
    if name == "profiles" {
        let mut out = vec![
            format!("-- {}", t.header()),
            "".into(),
            "---@type DotfileColorProfiles".into(),
            "return {".into(),
            format!(
                "  active = require {},",
                lua(&format!("ui.colors.{}", t.profile))
            ),
            "  profiles = {".into(),
        ];
        for theme in repo.themes.values() {
            out.push(format!(
                "    [{}] = require({}).colors,",
                lua(&theme.name),
                lua(&format!("ui.colors.{}", theme.profile))
            ));
        }
        out.extend(["  },".into(), "}".into()]);
        return Ok(lines(out));
    }
    if name == "_dotfile-theme" {
        return Ok(format!(
            "-- {}\n\n{}",
            t.header(),
            include_str!("wezterm-types.txt")
        ));
    }
    if name == "fonts" {
        return Ok(lines(vec![
            format!("-- {}", t.header()),
            "---@type DotfileFonts".into(),
            "return {".into(),
            format!("  font_size = {},", t.size("terminal")?),
            format!("  interface_font_size = {},", t.size("interface")?),
            format!("  nerd_family = {},", lua(t.font("nerd")?)),
            format!("  general_family = {},", lua(t.font("general")?)),
            "}".into(),
        ]));
    }
    Ok(lines(vec![
        format!("-- {}", t.header()),
        "---@type FontFamily".into(),
        "return {".into(),
        format!("  family = {},", lua(t.font(name)?)),
        "}".into(),
    ]))
}
