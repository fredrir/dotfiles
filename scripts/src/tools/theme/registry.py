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
    Emitter("kitty", emitters.emit_kitty, ["shared/kitty/colors-mocha.conf"]),
    Emitter(
        "konsole", emitters.emit_konsole, ["linux/kde/konsole/share/Catppuccin-Mocha.colorscheme"]
    ),
    Emitter("fastfetch-config", emitters.emit_fastfetch_config, ["shared/fastfetch/config.jsonc"]),
    Emitter("fastfetch-logo", emitters.emit_fastfetch_logo, ["shared/fastfetch/arch.txt"]),
    Emitter("starship", emitters.emit_starship, ["shared/starship/starship.toml"]),
    Emitter("zsh", emitters.emit_zsh, ["shared/zsh/conf.d/03-theme.zsh"]),
    Emitter("obsidian", emitters.emit_obsidian, [f"{emitters.OBSIDIAN_DIR}/theme.css"]),
    Emitter(
        "gtk",
        emitters.emit_gtk,
        [
            "linux/common/gtk/gtk-3.0/colors.css",
            "linux/common/gtk/gtk-4.0/colors.css",
        ],
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
