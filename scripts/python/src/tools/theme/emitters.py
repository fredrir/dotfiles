import colorsys
import glob
import os
import re
import sys

from tools.theme.model import (
    FONTS_FILE,
    Theme,
    list_profiles,
    load_map,
    load_toml,
    path,
    profile_palette,
)
from tools.theme.render import (
    replace_between,
    replace_ini_section,
    set_ini_key,
)

ANSI_NORMAL = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"]

FASTFETCH_SECTIONS = ["section_system", "section_hardware", "section_desktop", "section_network"]

FASTFETCH_CONFIGS = [
    "shared/fastfetch/config.jsonc",
    "linux/arch/fastfetch/config.jsonc",
    "linux/ubuntu/fastfetch/config.jsonc",
    "macos/fastfetch/config.jsonc",
]

FASTFETCH_LOGOS = [
    "linux/arch/fastfetch/arch.txt",
    "linux/ubuntu/fastfetch/ubuntu.txt",
    "macos/fastfetch/apple.txt",
]

PROMPT_ROLES = ("prompt_python", "prompt_git", "prompt_dir", "prompt_duration", "prompt_char")

EZA_KINDS = ("fi", "di", "ex", "ln", "pi", "so", "bd", "cd")

OBSIDIAN_DIR = "shared/obsidian/themes/Fredrir"

PANEL_PRESETS_DIR = "linux/kde/panel-colorizer/presets"

NVIM_CATPPUCCIN = "shared/nvim/lua/plugins/catppuccin.lua"

WEZTERM_COLORS_DIR = "shared/wezterm/ui/colors"

WEZTERM_FONTS_DIR = "shared/wezterm/ui/fonts"

WEZTERM_COLORS = (
    ("foreground", "foreground"),
    ("background", "background"),
    ("cursor_bg", "cursor"),
    ("cursor_fg", "cursor_text"),
    ("cursor_border", "cursor"),
    ("selection_fg", "selection_foreground"),
    ("selection_bg", "selection_background"),
)

LUA_IDENTIFIER = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")

LUA_KEYWORDS = frozenset(
    [
        "and",
        "break",
        "do",
        "else",
        "elseif",
        "end",
        "false",
        "for",
        "function",
        "goto",
        "if",
        "in",
        "local",
        "nil",
        "not",
        "or",
        "repeat",
        "return",
        "then",
        "true",
        "until",
        "while",
    ]
)

GTK_VERSIONS = ("gtk-3.0", "gtk-4.0")

QUICKLAUNCH_KEYS = (
    ("accent", "accent"),
    ("background", "view_bg"),
    ("text", "foreground"),
    ("muted", "inactive"),
    ("selection", "selection_bg"),
)


def _lua_string(value):
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _lua_key(name):
    if LUA_IDENTIFIER.match(name) and name not in LUA_KEYWORDS:
        return name
    return f"[{_lua_string(name)}]"


def _lua_entries(entries, indent):
    return [f"{indent}{_lua_key(key)} = {value}," for key, value in entries]


def wezterm_font_roles():
    return sorted(load_toml(FONTS_FILE)["fonts"])


def wezterm_outputs():
    targets = [f"{WEZTERM_COLORS_DIR}/{profile}.lua" for profile in list_profiles()]
    targets.append(f"{WEZTERM_COLORS_DIR}/init.lua")
    targets += [f"{WEZTERM_FONTS_DIR}/{role}.lua" for role in wezterm_font_roles()]
    targets.append(f"{WEZTERM_FONTS_DIR}/init.lua")
    return targets


def _wezterm_scheme(scheme):
    def ansi_list(names):
        return "{ " + ", ".join(_lua_string(scheme.hex(name)) for name in names) + " }"

    lines = [
        f"-- {scheme.header}",
        "return {",
        f"  name = {_lua_string(scheme.name)},",
        "  colors = {",
    ]
    lines += _lua_entries(
        [
            (key, _lua_string(scheme.hex(scheme.data["terminal"][role])))
            for key, role in WEZTERM_COLORS
        ],
        "    ",
    )
    lines.append(f"    ansi = {ansi_list(ANSI_NORMAL)},")
    lines.append(f"    brights = {ansi_list(['bright_' + name for name in ANSI_NORMAL])},")
    lines += ["  },", "}"]
    return "\n".join(lines) + "\n"


def _wezterm_colors_init(theme):
    lines = [f"-- {theme.header}", "local profiles = {"]
    lines += [f"  {_lua_string(profile)}," for profile in list_profiles()]
    lines += [
        "}",
        "",
        "local M = {}",
        "",
        "function M.apply_to_config(config)",
        "  local schemes = {}",
        "  for _, profile in ipairs(profiles) do",
        '    local scheme = require("ui.colors." .. profile)',
        "    schemes[scheme.name] = scheme.colors",
        "  end",
        "  config.color_schemes = schemes",
        f"  config.color_scheme = {_lua_string(theme.name)}",
        "end",
        "",
        "return M",
    ]
    return "\n".join(lines) + "\n"


def _wezterm_font(theme, role):
    lines = [f"-- {theme.header}", "return {", f"  family = {_lua_string(theme.font(role))},", "}"]
    return "\n".join(lines) + "\n"


def _wezterm_fonts_init(theme):
    terminal = f"platform.is_mac and {theme.size('terminal_mac')} or {theme.size('terminal')}"
    lines = [
        f"-- {theme.header}",
        'local wezterm = require "wezterm"',
        'local platform = require "utils.platform"',
        "",
        'local terminal = require "ui.fonts.nerd"',
        'local interface = require "ui.fonts.general"',
        "",
        "local M = {}",
        "",
        "function M.apply_to_config(config)",
        "  config.font = wezterm.font_with_fallback { terminal.family }",
        f"  config.font_size = {terminal}",
        "",
        "  config.window_frame = config.window_frame or {}",
        "  config.window_frame.font = wezterm.font_with_fallback { interface.family }",
        f"  config.window_frame.font_size = {theme.size('interface')}",
        "end",
        "",
        "return M",
    ]
    return "\n".join(lines) + "\n"


def emit_wezterm(theme, out):
    for profile in list_profiles():
        scheme = theme if profile == theme.profile else Theme.load(profile)
        out.write(path(WEZTERM_COLORS_DIR, f"{profile}.lua"), _wezterm_scheme(scheme))
    out.write(path(WEZTERM_COLORS_DIR, "init.lua"), _wezterm_colors_init(theme))
    for role in wezterm_font_roles():
        out.write(path(WEZTERM_FONTS_DIR, f"{role}.lua"), _wezterm_font(theme, role))
    out.write(path(WEZTERM_FONTS_DIR, "init.lua"), _wezterm_fonts_init(theme))


def emit_fastfetch_config(theme, out):
    lines = []
    for role in FASTFETCH_SECTIONS:
        value = theme.role(role)
        lines.append(
            f'"{theme.truecolor(value, bold=True)}", // {theme.data["roles"][role]} {value}'
        )
    separator = theme.role("separator")
    lines.append(
        f'"{theme.truecolor(separator)}" // {theme.data["roles"]["separator"]} {separator}'
    )

    def transform(text):
        text = replace_between(text, "constants", lines)
        updated = text.split("\n")
        for index, line in enumerate(updated):
            if "theme:separator" in line:
                updated[index] = re.sub(r"#[0-9a-fA-F]{6}", separator, line)
        return "\n".join(updated)

    for config in FASTFETCH_CONFIGS:
        out.edit(path(config), transform)


def emit_fastfetch_logo(theme, out):
    stops = [theme.rgb(theme.role(role)) for role in FASTFETCH_SECTIONS]
    segments = len(stops) - 1

    def lerp(start, end, ratio):
        return tuple(round(start[i] + (end[i] - start[i]) * ratio) for i in range(3))

    for logo in FASTFETCH_LOGOS:
        target = path(logo)
        with open(target, encoding="utf-8") as handle:
            raw = handle.read().split("\n")
        trailing_newline = bool(raw) and raw[-1] == ""
        if trailing_newline:
            raw = raw[:-1]

        art = [re.sub(r"\x1b\[[0-9;]*m", "", line) for line in raw]
        count = len(art)

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
    lines = [f"# {theme.header}", "[palettes.theme]"]
    # A width per run of entries, because the blank line below starts a second
    # one and `align_entries` lines each up on its own longest key.
    width = max(len(name) for name in names)
    for name in names:
        lines.append(f"{name.ljust(width)} = '{theme.hex(name)}'")
    lines.append("")
    width = max(len(role) for role in PROMPT_ROLES)
    for role in PROMPT_ROLES:
        lines.append(f"{role.ljust(width)} = '{theme.role(role)}'")
    out.edit(
        path("shared/starship/starship.toml"),
        lambda text: replace_between(text, "palette", lines),
    )


def emit_zsh(theme, out):
    def escape(role):
        return f"$'\\e[{theme.ansi(theme.role(role))}m'"

    lines = [
        f"# {theme.header}",
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
            for extension in extensions[category].split():
                parts.append(f"*.{extension}={theme.ansi(theme.hex(color))}")
        for key, color in eza.items():
            if key.startswith("*"):
                parts.append(f"{key}={theme.ansi(theme.hex(color))}")
        lines.append("unset LS_COLORS")
        lines.append(f'export EZA_COLORS="{":".join(parts)}"')
    out.write(path("shared/zsh/conf.d/03-theme.zsh"), "\n".join(lines) + "\n")


def obsidian_derived(theme):
    source = load_map("obsidian")["derived"]["source"]
    red, green, blue = (channel / 255 for channel in theme.rgb(theme.hex(source)))
    hue, lightness, saturation = colorsys.rgb_to_hls(red, green, blue)
    degrees = round(hue * 360)
    percent_s = round(saturation * 100)
    percent_l = round(lightness * 100)
    return {
        "accent_h": str(degrees),
        "accent_s": f"{percent_s}%",
        "accent_l": f"{percent_l}%",
        "accent_hsl": f"{degrees}, {percent_s}%, {percent_l}%",
    }


def obsidian_variables(theme, derived):
    lines = []
    for name, value in load_map("obsidian")["variables"].items():
        if isinstance(value, str):
            lines.append(f"{name}: {theme.hex(value)};")
        elif "literal" in value:
            lines.append(f"{name}: {value['literal']};")
        elif "derived" in value:
            lines.append(f"{name}: {derived[value['derived']]};")
        elif "rgb" in value:
            channels = ", ".join(str(channel) for channel in theme.rgb(theme.hex(value["rgb"])))
            lines.append(f"{name}: {channels};")
        else:
            channels = ", ".join(str(channel) for channel in theme.rgb(theme.hex(value["color"])))
            lines.append(f"{name}: rgba({channels}, {value['alpha']});")
    return lines


def emit_obsidian(theme, out):
    lines = [f"color-scheme: {'dark' if theme.dark else 'light'};"]
    lines += obsidian_variables(theme, obsidian_derived(theme))
    if theme.uses_fonts("obsidian"):
        general = theme.font("general").replace("\\", "\\\\").replace('"', '\\"')
        nerd = theme.font("nerd").replace("\\", "\\\\").replace('"', '\\"')
        lines += [
            f'--font-interface-theme: "{general}", sans-serif;',
            f'--font-text-theme: "{general}", sans-serif;',
            f'--font-monospace-theme: "{nerd}", ui-monospace, monospace;',
        ]
    out.edit(
        path(OBSIDIAN_DIR, "theme.css"),
        lambda text: replace_between(text, "variables", lines),
    )


def emit_nvim(theme, out):
    spec = load_map("nvim")
    flavour = spec["flavour"]["dark" if theme.dark else "light"]
    colors = {name: theme.hex(value) for name, value in spec["colors"].items()}

    def transform(text):
        unit = "\t" if "\n\t" in text else "  "
        lines = [f'flavour = "{flavour}",', "color_overrides = {", f"{unit}all = {{"]
        lines += [f'{unit * 2}{name} = "{value}",' for name, value in colors.items()]
        lines += [f"{unit}}},", "},"]
        return replace_between(text, "palette", lines)

    out.edit(path(NVIM_CATPPUCCIN), transform)


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
                base = variable.removesuffix("_breeze")
                if base in mapping:
                    line = f"{match.group(1)}{variable}{match.group(3)}{theme.color(mapping[base])}{match.group(5)}"
                else:
                    sys.stderr.write(f"dotfile theme: unmapped GTK color '{variable}'\n")
            lines.append(line)
        return "\n".join(lines)

    for version in GTK_VERSIONS:
        out.edit(path(f"linux/common/gtk/{version}/colors.css"), transform)


def emit_gtk_settings(theme, out):
    font = f"{theme.font('general')},  {theme.size('interface')}"
    prefer_dark = "true" if theme.dark else "false"

    for version in GTK_VERSIONS:
        target = f"linux/common/gtk/{version}/settings.ini"

        def transform(text, where=target):
            text = set_ini_key(text, "Settings", "gtk-font-name", font, where)
            text = set_ini_key(
                text, "Settings", "gtk-application-prefer-dark-theme", prefer_dark, where
            )
            return set_ini_key(text, "Settings", "gtk-icon-theme-name", theme.icons, where)

        out.edit(path(target), transform)


def _hex_to_name(theme):
    mapping = {}
    active = theme.profile
    ordered = [active] + [name for name in list_profiles() if name != active]
    # An ANSI palette may repeat a hex across slots, so first writer wins and
    # the active profile is scanned first to make its own names authoritative.
    for profile in ordered:
        for name, value in profile_palette(profile).items():
            mapping.setdefault(value.lstrip("#").lower(), name)
    for key, name in load_map("catppuccin")["colors"].items():
        mapping.setdefault(key.lower(), name)
    return mapping


def _remap_hex(theme, text, mapping):
    pattern = re.compile(r"#([0-9a-fA-F]{8}|[0-9a-fA-F]{6})")

    def replace(match):
        token = match.group(1).lower()
        if len(token) == 6 and token in mapping:
            return theme.hex(mapping[token])
        return match.group(0)

    return pattern.sub(replace, text)


def panel_preset_files():
    found = sorted(glob.glob(path(PANEL_PRESETS_DIR, "*", "settings.json")))
    return [target for target in found if os.path.getsize(target)]


def emit_panel_presets(theme, out):
    mapping = _hex_to_name(theme)
    for target in panel_preset_files():
        out.edit(target, lambda text: _remap_hex(theme, text, mapping))


# The target is symlinked over the live desktop, and KConfig treats it as the
# whole config rather than an overlay, so a key we drop is a key plasma boots
# without. Only state that satisfies all three tests is left out when recapturing
# it: the desktop writes it with no user action, its value is unstable, and
# plasma re-derives a working one from nothing. Today that is DialogHeight and
# DialogWidth, popupHeight and popupWidth, LastVideo and LastVideoPosition, and
# the ItemGeometries* family. Everything else stays, including empty values -
# `launchers=` means no pinned launchers, while an absent one means the defaults.
def emit_desktop_appletsrc(theme, out):
    mapping = _hex_to_name(theme)
    rgb_map = {}
    for token, name in mapping.items():
        rgb_map[theme.rgb_csv("#" + token)] = theme.rgb_csv(theme.hex(name))
    pattern = re.compile(r"^([^=\[]+)=(\d{1,3},\d{1,3},\d{1,3})$")

    def transform(text):
        lines = []
        for line in _remap_hex(theme, text, mapping).split("\n"):
            match = pattern.match(line)
            if match and match.group(2) in rgb_map:
                lines.append(f"{match.group(1)}={rgb_map[match.group(2)]}")
            else:
                lines.append(line)
        return "\n".join(lines)

    out.edit(path("linux/kde/plasma/plasma-org.kde.plasma.desktop-appletsrc"), transform)


def emit_quicklaunch(theme, out):
    width = max(len(key) for key, _ in QUICKLAUNCH_KEYS)
    lines = [f"# {theme.header}"]
    for key, role in QUICKLAUNCH_KEYS:
        lines.append(f'{key.ljust(width)} = "{theme.kde(role)}"')
    out.edit(
        path("linux/common/quicklaunch/config.toml"),
        lambda text: replace_between(text, "quicklaunch", lines),
    )
