mod apps;
pub(crate) mod tmux;
mod wezterm;
use super::{
    Result,
    model::{Repository, Theme},
};
use std::fs;
#[derive(Clone, Debug)]
pub enum Kind {
    Wezterm,
    Tmux,
    FastfetchConfig,
    FastfetchLogo,
    Starship,
    Zsh,
    Obsidian,
    Nvim,
    Yazi,
    YaziSnapshot(String),
    Contrast(String),
    Gtk,
    GtkSettings,
    Quicklaunch,
    Panel,
    Plasma,
    Desktop,
}
#[derive(Clone, Debug)]
pub struct Target {
    pub path: String,
    pub kind: Kind,
    pub staged: bool,
}
pub fn targets(repo: &Repository) -> Result<Vec<Target>> {
    let mut output = Vec::new();
    let mut add =
        |path: String, kind: Kind, staged: bool| output.push(Target { path, kind, staged });
    for name in repo.names() {
        add(
            format!("shared/wezterm/ui/colors/{name}.lua"),
            Kind::Wezterm,
            true,
        );
    }
    add(
        "shared/wezterm/ui/colors/profiles.lua".into(),
        Kind::Wezterm,
        true,
    );
    let mut fonts = super::model::table(&repo.data.fonts["fonts"])?
        .keys()
        .collect::<Vec<_>>();
    fonts.sort();
    for name in fonts {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains(['/', '\\'])
            || name.chars().any(char::is_control)
        {
            return Err(format!("invalid font role filename: {name:?}"));
        }
        add(
            format!("shared/wezterm/ui/fonts/{name}.lua"),
            Kind::Wezterm,
            true,
        );
    }
    for path in [
        "shared/wezterm/ui/fonts/fonts.lua",
        "shared/wezterm/_types/_dotfile-theme.lua",
    ] {
        add(path.into(), Kind::Wezterm, true);
    }
    add("shared/tmux/theme.conf".into(), Kind::Tmux, true);
    for group in ["shared", "linux/arch", "linux/ubuntu", "macos"] {
        add(
            format!("{group}/fastfetch/config.jsonc"),
            Kind::FastfetchConfig,
            true,
        );
    }
    for (group, name) in [
        ("linux/arch", "arch"),
        ("linux/ubuntu", "ubuntu"),
        ("macos", "apple"),
    ] {
        add(
            format!("{group}/fastfetch/{name}.txt"),
            Kind::FastfetchLogo,
            true,
        );
    }
    for (path, kind) in [
        ("shared/starship/starship.toml", Kind::Starship),
        ("shared/zsh/conf.d/03-theme.zsh", Kind::Zsh),
        ("shared/obsidian/themes/Fredrir/theme.css", Kind::Obsidian),
        ("shared/nvim/lua/plugins/catppuccin.lua", Kind::Nvim),
        ("shared/yazi/theme.toml", Kind::Yazi),
    ] {
        add(path.into(), kind, true);
    }
    for name in repo.names() {
        add(
            format!("theme/snapshots/yazi/{name}.toml"),
            Kind::YaziSnapshot(name),
            true,
        );
    }
    for name in repo.names() {
        add(
            format!("theme/contrast/{name}.md"),
            Kind::Contrast(name),
            true,
        );
    }
    for version in ["gtk-3.0", "gtk-4.0"] {
        add(
            format!("linux/common/gtk/{version}/colors.css"),
            Kind::Gtk,
            true,
        );
    }
    for version in ["gtk-3.0", "gtk-4.0"] {
        add(
            format!("linux/common/gtk/{version}/settings.ini"),
            Kind::GtkSettings,
            true,
        );
    }
    add(
        "linux/common/quicklaunch/config.toml".into(),
        Kind::Quicklaunch,
        true,
    );
    let presets = repo.root.join("linux/kde/panel-colorizer/presets");
    let mut paths = Vec::new();
    match fs::read_dir(&presets) {
        Ok(entries) => {
            for entry in entries {
                let path = entry
                    .map_err(|e| e.to_string())?
                    .path()
                    .join("settings.json");
                if path.is_file() && fs::metadata(&path).map_err(|e| e.to_string())?.len() > 0 {
                    paths.push(path);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", presets.display())),
    }
    paths.sort();
    for path in paths {
        add(
            path.strip_prefix(&repo.root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned(),
            Kind::Panel,
            true,
        );
    }
    add("linux/kde/plasma/kdeglobals".into(), Kind::Plasma, false);
    add(
        "linux/kde/plasma/plasma-org.kde.plasma.desktop-appletsrc".into(),
        Kind::Desktop,
        false,
    );
    Ok(output)
}
pub fn emit(repo: &Repository, theme: &Theme, target: &Target) -> Result<String> {
    match &target.kind {
        Kind::Wezterm => wezterm::render(repo, theme, &target.path),
        Kind::Tmux => tmux::render(theme),
        Kind::Yazi => super::validate::yazi_render(theme),
        Kind::YaziSnapshot(name) => super::validate::yazi_render(repo.theme(name)?),
        Kind::Contrast(name) => super::validate::matrix(repo.theme(name)?),
        Kind::Zsh => apps::zsh(theme),
        _ => {
            let previous = fs::read_to_string(repo.root.join(&target.path))
                .map_err(|e| format!("{}: {e}", target.path))?;
            apps::render(repo, theme, &target.kind, &previous)
                .map_err(|e| format!("{}: {e}", target.path))
        }
    }
}
