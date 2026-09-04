from tools.theme.emitters import (
    contrast,
    fastfetch,
    gtk,
    nvim,
    obsidian,
    panel_colorizer,
    plasma,
    quicklaunch,
    starship,
    wezterm,
    yazi,
    zsh,
)


class Emitter:
    def __init__(self, name, run, outputs, staged=True):
        self.name = name
        self.run = run
        self._outputs = outputs
        self.staged = staged

    def outputs(self):
        produced = self._outputs() if callable(self._outputs) else self._outputs
        return list(produced)


EMITTERS = [
    Emitter("wezterm", wezterm.emit, wezterm.outputs),
    Emitter("fastfetch-config", fastfetch.emit_config, fastfetch.CONFIGS),
    Emitter("fastfetch-logo", fastfetch.emit_logo, fastfetch.LOGOS),
    Emitter("starship", starship.emit, [starship.OUTPUT]),
    Emitter("zsh", zsh.emit, [zsh.OUTPUT]),
    Emitter("obsidian", obsidian.emit, [obsidian.OUTPUT]),
    Emitter("nvim", nvim.emit, [nvim.OUTPUT]),
    Emitter("yazi", yazi.emit, [yazi.OUTPUT]),
    Emitter("yazi-snapshots", yazi.emit_snapshots, yazi.snapshot_outputs),
    Emitter("contrast-matrices", contrast.emit, contrast.outputs),
    Emitter("gtk", gtk.emit_colors, gtk.color_outputs),
    Emitter("gtk-settings", gtk.emit_settings, gtk.settings_outputs),
    Emitter("quicklaunch", quicklaunch.emit, [quicklaunch.OUTPUT]),
    Emitter("panel-presets", panel_colorizer.emit, panel_colorizer.outputs),
    Emitter(
        "kde-colorscheme",
        plasma.emit_colorscheme,
        [plasma.KDEGLOBALS],
        staged=False,
    ),
    Emitter(
        "desktop-appletsrc",
        plasma.emit_desktop_appletsrc,
        [plasma.DESKTOP_APPLETSRC],
        staged=False,
    ),
]
