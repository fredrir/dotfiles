import colorsys
import glob
import os
import re
import sys

from .model import GENERATED_HEADER, load_map, load_template, path
from .render import replace_between, replace_ini_section, set_ini_key

KITTY_SLOTS = {
    "black": 0, "red": 1, "green": 2, "yellow": 3, "blue": 4, "magenta": 5,
    "cyan": 6, "white": 7, "bright_black": 8, "bright_red": 9,
    "bright_green": 10, "bright_yellow": 11, "bright_blue": 12,
    "bright_magenta": 13, "bright_cyan": 14, "bright_white": 15,
}

ANSI_NORMAL = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"]

FASTFETCH_SECTIONS = ["section_system", "section_hardware", "section_desktop", "section_network"]

PROMPT_ROLES = ("prompt_python", "prompt_git", "prompt_dir", "prompt_duration", "prompt_char")

EZA_KINDS = ("fi", "di", "ex", "ln", "pi", "so", "bd", "cd")

OBSIDIAN_DIR = "linux/common/obsidian/themes/Fredrir"

OBSIDIAN_RGB_ROLES = (
    "crust", "overlay", "mauve", "red", "peach", "orange",
    "yellow", "green", "teal", "blue", "pink",
)

PANEL_PRESETS_DIR = "linux/kde/panel-colorizer/presets"

QUICKLAUNCH_KEYS = (
    ("accent", "accent"),
    ("background", "view_bg"),
    ("text", "foreground"),
    ("muted", "inactive"),
    ("selection", "selection_bg"),
)


def emit_kitty(theme, out):
    terminal = theme.data["terminal"]
    ansi = terminal["ansi"]
    tabs = theme.data["kitty"]["tabs"]
    lines = [
        f"# {GENERATED_HEADER}",
        f"# {theme.data['name']}",
        "",
        f"foreground              {theme.hex(terminal['foreground'])}",
        f"background              {theme.hex(terminal['background'])}",
        f"selection_foreground    {theme.hex(terminal['selection_foreground'])}",
        f"selection_background    {theme.hex(terminal['selection_background'])}",
        f"cursor                  {theme.hex(terminal['cursor'])}",
        f"cursor_text_color       {theme.hex(terminal['cursor_text'])}",
        f"url_color               {theme.hex(terminal['url'])}",
        "",
    ]
    for name, index in KITTY_SLOTS.items():
        lines.append(f"color{index:<2} {theme.hex(ansi[name])}")
    lines += [
        "",
        f"active_tab_foreground   {theme.hex(tabs['active_foreground'])}",
        f"active_tab_background   {theme.hex(tabs['active_background'])}",
        f"inactive_tab_foreground {theme.hex(tabs['inactive_foreground'])}",
        f"inactive_tab_background {theme.hex(tabs['inactive_background'])}",
        f"tab_bar_background      {theme.hex(tabs['bar_background'])}",
    ]
    out.write(path("shared/kitty/colors-mocha.conf"), "\n".join(lines) + "\n")


def emit_konsole(theme, out):
    terminal = theme.data["terminal"]
    ansi = terminal["ansi"]
    bright = ["bright_" + name for name in ANSI_NORMAL]

    def block(section, value):
        return [f"[{section}]", f"Color={theme.rgb_csv(value)}", ""]

    body = []
    body += block("Background", theme.hex(terminal["background"]))
    body += block("BackgroundIntense", theme.hex(terminal["background"]))
    body += block("BackgroundFaint", theme.hex(terminal["background"]))
    for index in range(8):
        body += block(f"Color{index}", theme.hex(ansi[ANSI_NORMAL[index]]))
        body += block(f"Color{index}Intense", theme.hex(ansi[bright[index]]))
        body += block(f"Color{index}Faint", theme.hex(ansi[ANSI_NORMAL[index]]))
    body += block("Foreground", theme.hex(terminal["foreground"]))
    body += block("ForegroundIntense", theme.hex(terminal["foreground"]))
    body += block("ForegroundFaint", theme.hex(terminal["foreground"]))
    body += [
        "[General]",
        "Blur=false",
        "ColorRandomization=false",
        f"Description={theme.data['name']}",
        "Opacity=1",
        "Wallpaper=",
    ]
    content = f"# {GENERATED_HEADER}\n" + "\n".join(body) + "\n"
    out.write(path("linux/kde/konsole/share/Catppuccin-Mocha.colorscheme"), content)


def emit_fastfetch_config(theme, out):
    lines = []
    for role in FASTFETCH_SECTIONS:
        value = theme.role(role)
        lines.append(f'"{theme.truecolor(value, bold=True)}", // {theme.data["roles"][role]} {value}')
    separator = theme.role("separator")
    lines.append(f'"{theme.truecolor(separator)}" // {theme.data["roles"]["separator"]} {separator}')

    def transform(text):
        text = replace_between(text, "constants", lines, indent=" " * 6)
        updated = text.split("\n")
        for index, line in enumerate(updated):
            if "theme:separator" in line:
                updated[index] = re.sub(r"#[0-9a-fA-F]{6}", separator, line)
        return "\n".join(updated)

    out.edit(path("shared/fastfetch/config.jsonc"), transform)


def emit_fastfetch_logo(theme, out):
    stops = [theme.rgb(theme.role(role)) for role in FASTFETCH_SECTIONS]
    segments = len(stops) - 1
    target = path("shared/fastfetch/arch.txt")
    with open(target, encoding="utf-8") as handle:
        raw = handle.read().split("\n")
    trailing_newline = bool(raw) and raw[-1] == ""
    if trailing_newline:
        raw = raw[:-1]

    art = [re.sub(r"\x1b\[[0-9;]*m", "", line) for line in raw]
    count = len(art)

    def lerp(start, end, ratio):
        return tuple(round(start[i] + (end[i] - start[i]) * ratio) for i in range(3))

    body = []
    for index, line in enumerate(art):
        position = (index / (count - 1)) * segments if count > 1 else 0
        segment = min(int(position), segments - 1)
        red, green, blue = lerp(stops[segment], stops[segment + 1], position - segment)
        body.append(f"\x1b[1;38;2;{red};{green};{blue}m{line}")
    content = "\n".join(body) + "\x1b[0m"
    if trailing_newline:
        content += "\n"
    out.write(target, content)


def emit_starship(theme, out):
    names = list(theme.palette.keys())
    width = max(len(name) for name in names + list(PROMPT_ROLES))
    lines = [f"# {GENERATED_HEADER}", "[palettes.mocha]"]
    for name in names:
        lines.append(f"{name.ljust(width)} = '{theme.hex(name)}'")
    lines.append("")
    for role in PROMPT_ROLES:
        lines.append(f"{role.ljust(width)} = '{theme.role(role)}'")
    out.edit(
        path("shared/starship/starship.toml"),
        lambda text: replace_between(text, "palette", lines),
    )


def emit_zsh(theme, out):
    def escape(role):
        return "$'\\e[{}m'".format(theme.ansi(theme.role(role)))

    lines = [
        f"# {GENERATED_HEADER}",
        "export THEME_RESET=$'\\e[0m'",
        f"export THEME_SUDO={escape('sudo')}",
        f"export THEME_GIT={escape('prompt_git')}",
        f"export THEME_DIR={escape('prompt_dir')}",
        f"export THEME_CHAR={escape('prompt_char')}",
    ]
    eza = dict(theme.data.get("eza", {}))
    if eza:
        categories = eza.pop("categories", {})
        extensions = load_map("eza")["categories"]
        parts = ["reset"]
        for kind in EZA_KINDS:
            if kind in eza:
                parts.append(f"{kind}={theme.ansi(theme.hex(eza[kind]))}")
        for category, color in categories.items():
            if category not in extensions:
                sys.stderr.write(f"theme: unknown [eza.categories] group '{category}'\n")
                continue
            for extension in extensions[category].split():
                parts.append(f"*.{extension}={theme.ansi(theme.hex(color))}")
        for key, color in eza.items():
            if key.startswith("*"):
                parts.append(f"{key}={theme.ansi(theme.hex(color))}")
        lines.append("unset LS_COLORS")
        lines.append(f'export EZA_COLORS="{":".join(parts)}"')
    out.write(path("shared/zsh/conf.d/03-theme.zsh"), "\n".join(lines) + "\n")


def emit_obsidian(theme, out):
    values = dict(theme.palette)
    for name in OBSIDIAN_RGB_ROLES:
        values[f"{name}_rgb"] = ", ".join(str(channel) for channel in theme.rgb(theme.hex(name)))
    red, green, blue = (channel / 255 for channel in theme.rgb(theme.hex("mauve")))
    hue, lightness, saturation = colorsys.rgb_to_hls(red, green, blue)
    values["mauve_h"] = str(round(hue * 360))
    values["mauve_s"] = f"{round(saturation * 100)}%"
    values["mauve_l"] = f"{round(lightness * 100)}%"
    values["mauve_hsl"] = f"{round(hue * 360)}, {round(saturation * 100)}%, {round(lightness * 100)}%"

    overrides = []
    if theme.uses_fonts("obsidian"):
        general = theme.font("general").replace("\\", "\\\\").replace('"', '\\"')
        nerd = theme.font("nerd").replace("\\", "\\\\").replace('"', '\\"')
        overrides = [
            f'  --font-interface-theme: "{general}", sans-serif;',
            f'  --font-text-theme: "{general}", sans-serif;',
            f'  --font-monospace-theme: "{nerd}", ui-monospace, monospace;',
        ]
    values["font_overrides"] = "\n".join(overrides)
    if overrides:
        values["font_overrides"] += "\n"

    css = load_template("obsidian.css")
    for name, value in values.items():
        css = css.replace(f"@{name}@", value)
    unresolved = re.findall(r"@[a-z0-9_]+@", css)
    if unresolved:
        raise SystemExit(f"unresolved Obsidian theme values: {', '.join(unresolved)}")

    out.write(path(OBSIDIAN_DIR, "theme.css"), css)
    out.write(path(OBSIDIAN_DIR, "manifest.json"), load_template("obsidian-manifest.json"))


def emit_kde_colorscheme(theme, out):
    spec = load_map("kde")
    groups = spec["groups"]
    foregrounds = spec["foregrounds"]
    selection = spec["selection_foregrounds"]

    sections = {}
    for group, (background, alternate) in groups.items():
        overrides = selection if group == "Colors:Selection" else {}
        body = [
            f"BackgroundAlternate={theme.rgb_csv(theme.kde(alternate))}",
            f"BackgroundNormal={theme.rgb_csv(theme.kde(background))}",
            f"DecorationFocus={theme.rgb_csv(theme.kde('decoration'))}",
            f"DecorationHover={theme.rgb_csv(theme.kde('decoration'))}",
        ]
        for key, role in foregrounds.items():
            body.append(f"{key}={theme.rgb_csv(theme.kde(overrides.get(key, role)))}")
        sections[group] = body
    sections["WM"] = [
        f"activeBackground={theme.rgb_csv(theme.kde('wm_active_bg'))}",
        f"activeBlend={theme.rgb_csv(theme.kde('wm_active_blend'))}",
        f"activeForeground={theme.rgb_csv(theme.kde('wm_active_fg'))}",
        f"inactiveBackground={theme.rgb_csv(theme.kde('wm_inactive_bg'))}",
        f"inactiveBlend={theme.rgb_csv(theme.kde('wm_inactive_blend'))}",
        f"inactiveForeground={theme.rgb_csv(theme.kde('wm_inactive_fg'))}",
    ]
    accent = theme.rgb_csv(theme.kde("accent"))

    def transform(text):
        for header, body in sections.items():
            text = replace_ini_section(text, header, body)
        text = set_ini_key(text, "General", "AccentColor", accent)
        return set_ini_key(text, "General", "LastUsedCustomAccentColor", accent)

    out.edit(path("linux/kde/plasma/kdeglobals"), transform)


def emit_gtk(theme, out):
    mapping = load_map("gtk")["colors"]
    pattern = re.compile(r"(@define-color\s+)(\S+)(\s+)(#[0-9a-fA-F]{6,8})(;.*)$")

    def transform(text):
        lines = []
        for line in text.split("\n"):
            match = pattern.match(line)
            if match:
                variable = match.group(2)
                base = variable[:-7] if variable.endswith("_breeze") else variable
                if base in mapping:
                    line = f"{match.group(1)}{variable}{match.group(3)}{theme.color(mapping[base])}{match.group(5)}"
                else:
                    sys.stderr.write(f"theme: unmapped GTK color '{variable}'\n")
            lines.append(line)
        return "\n".join(lines)

    for version in ("gtk-3.0", "gtk-4.0"):
        out.edit(path(f"linux/common/gtk/{version}/colors.css"), transform)


def _catppuccin_hex_map(theme):
    return {source: theme.hex(role) for source, role in load_map("catppuccin")["colors"].items()}


def _remap_hex(theme, text):
    mapping = _catppuccin_hex_map(theme)
    pattern = re.compile(r"#([0-9a-fA-F]{8}|[0-9a-fA-F]{6})")

    def replace(match):
        token = match.group(1).lower()
        return mapping[token] if len(token) == 6 and token in mapping else match.group(0)

    return pattern.sub(replace, text)


def panel_preset_files():
    found = sorted(glob.glob(path(PANEL_PRESETS_DIR, "*", "settings.json")))
    return [target for target in found if os.path.getsize(target)]


def emit_panel_presets(theme, out):
    for target in panel_preset_files():
        out.edit(target, lambda text: _remap_hex(theme, text))


def emit_desktop_appletsrc(theme, out):
    rgb_map = {}
    for source, role in load_map("catppuccin")["colors"].items():
        rgb_map[theme.rgb_csv("#" + source)] = theme.rgb_csv(theme.hex(role))
    pattern = re.compile(r"^([^=\[]+)=(\d{1,3},\d{1,3},\d{1,3})$")

    def transform(text):
        lines = []
        for line in _remap_hex(theme, text).split("\n"):
            match = pattern.match(line)
            if match and match.group(2) in rgb_map:
                lines.append(f"{match.group(1)}={rgb_map[match.group(2)]}")
            else:
                lines.append(line)
        return "\n".join(lines)

    out.edit(path("linux/kde/plasma/plasma-org.kde.plasma.desktop-appletsrc"), transform)


def emit_quicklaunch(theme, out):
    width = max(len(key) for key, _ in QUICKLAUNCH_KEYS)
    lines = [f"# {GENERATED_HEADER}"]
    for key, role in QUICKLAUNCH_KEYS:
        lines.append(f'{key.ljust(width)} = "{theme.kde(role)}"')
    out.edit(
        path("linux/common/quicklaunch/config.toml"),
        lambda text: replace_between(text, "quicklaunch", lines),
    )
