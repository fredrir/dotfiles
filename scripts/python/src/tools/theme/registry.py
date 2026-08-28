import os

from tools.theme import emitters
from tools.theme.model import ROOT


class Emitter:
    def __init__(self, name, run, outputs, stageable=True):
        self.name = name
        self.run = run
        self._outputs = outputs
        self.stageable = stageable

    def outputs(self):
        produced = self._outputs() if callable(self._outputs) else self._outputs
        return list(produced)


def _panel_preset_outputs():
    return [os.path.relpath(target, ROOT) for target in emitters.panel_preset_files()]


EMITTERS = [
    # Emitter("wezterm", emitters.emit_wezterm, ["shared/wezterm/wez/theme.lua"]),
    Emitter("fastfetch-config", emitters.emit_fastfetch_config, emitters.FASTFETCH_CONFIGS),
    Emitter("fastfetch-logo", emitters.emit_fastfetch_logo, emitters.FASTFETCH_LOGOS),
    Emitter("starship", emitters.emit_starship, ["shared/starship/starship.toml"]),
    Emitter("zsh", emitters.emit_zsh, ["shared/zsh/conf.d/03-theme.zsh"]),
    Emitter("obsidian", emitters.emit_obsidian, [f"{emitters.OBSIDIAN_DIR}/theme.css"]),
    Emitter("nvim", emitters.emit_nvim, [emitters.NVIM_CATPPUCCIN]),
    Emitter(
        "gtk",
        emitters.emit_gtk,
        [f"linux/common/gtk/{version}/colors.css" for version in emitters.GTK_VERSIONS],
    ),
    Emitter(
        "gtk-settings",
        emitters.emit_gtk_settings,
        [f"linux/common/gtk/{version}/settings.ini" for version in emitters.GTK_VERSIONS],
    ),
    Emitter("quicklaunch", emitters.emit_quicklaunch, ["linux/common/quicklaunch/config.toml"]),
    Emitter("panel-presets", emitters.emit_panel_presets, _panel_preset_outputs),
    Emitter(
        "kde-colorscheme",
        emitters.emit_kde_colorscheme,
        ["linux/kde/plasma/kdeglobals"],
        stageable=False,
    ),
    Emitter(
        "desktop-appletsrc",
        emitters.emit_desktop_appletsrc,
        ["linux/kde/plasma/plasma-org.kde.plasma.desktop-appletsrc"],
        stageable=False,
    ),
]
