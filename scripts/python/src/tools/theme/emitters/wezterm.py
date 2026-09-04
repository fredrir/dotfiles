import re

from tools.theme.model import FONTS_FILE, Theme, list_profiles, load_toml, path

COLORS_DIR = "shared/wezterm/ui/colors"
FONTS_DIR = "shared/wezterm/ui/fonts"
TYPES_FILE = "shared/wezterm/_types/_dotfile-theme.lua"

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


def _lua_string(value):
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _lua_key(name):
    if LUA_IDENTIFIER.match(name) and name not in LUA_KEYWORDS:
        return name
    return f"[{_lua_string(name)}]"


def _lua_entries(entries, indent):
    return [f"{indent}{_lua_key(key)} = {value}," for key, value in entries]


def font_roles():
    return sorted(load_toml(FONTS_FILE)["fonts"])


def outputs():
    targets = [f"{COLORS_DIR}/{profile}.lua" for profile in list_profiles()]
    targets.append(f"{COLORS_DIR}/profiles.lua")
    targets += [f"{FONTS_DIR}/{role}.lua" for role in font_roles()]
    targets.append(f"{FONTS_DIR}/fonts.lua")
    targets.append(TYPES_FILE)
    return targets


def _scheme(scheme):
    def color_list(colors):
        return "{ " + ", ".join(_lua_string(color) for color in colors) + " }"

    lines = [
        f"-- {scheme.header}",
        "",
        "---@type ColorProfile",
        "return {",
        f"  name = {_lua_string(scheme.name)},",
        "  colors = {",
    ]
    lines += _lua_entries(
        [(key, _lua_string(color)) for key, color in scheme.profile_data["ui"].items()],
        "    ",
    )
    lines += _lua_entries(
        [
            ("ansi", color_list(scheme.profile_data["ansi"]["normal"].values())),
            ("brights", color_list(scheme.profile_data["ansi"]["bright"].values())),
        ],
        "    ",
    )
    lines += ["  },", "}"]
    return "\n".join(lines) + "\n"


def _types(theme):
    lines = [
        f"-- {theme.header}",
        "",
        "---@class DotfileThemeColors",
        "---@field background string",
        "---@field primary string",
        "---@field accent string",
        "---@field surface string",
        "---@field foreground string",
        "---@field ansi string[]",
        "---@field brights string[]",
        "",
        "---@class ColorProfile",
        "---@field name string",
        "---@field colors DotfileThemeColors",
        "",
        "---@alias DotfileColorProfiles { active: ColorProfile, profiles: table<string, DotfileThemeColors> }",
        "",
        "---@class FontFamily",
        "---@field family string",
        "",
        "---@class DotfileFonts",
        "---@field font_size number",
        "---@field interface_font_size number",
        "---@field nerd_family string",
        "---@field general_family string",
    ]
    return "\n".join(lines) + "\n"


def _color_profile_entry(theme, profile):
    scheme = theme if profile == theme.profile else Theme.load(profile)
    module = _lua_string(f"ui.colors.{profile}")
    return f"    [{_lua_string(scheme.name)}] = require({module}).colors,"


def _color_profiles(theme):
    lines = [
        f"-- {theme.header}",
        "",
        "---@type DotfileColorProfiles",
        "return {",
        f'  active = require "ui.colors.{theme.profile}",',
        "  profiles = {",
    ]
    lines += [_color_profile_entry(theme, profile) for profile in list_profiles()]
    lines += [
        "  },",
        "}",
    ]
    return "\n".join(lines) + "\n"


def _font(theme, role):
    lines = [
        f"-- {theme.header}",
        "---@type FontFamily",
        "return {",
        f"  family = {_lua_string(theme.font(role))},",
        "}",
    ]
    return "\n".join(lines) + "\n"


def _fonts(theme):
    lines = [
        f"-- {theme.header}",
        "---@type DotfileFonts",
        "return {",
        f"  font_size = {theme.size('terminal')},",
        f"  interface_font_size = {theme.size('interface')},",
        f"  nerd_family = {_lua_string(theme.font('nerd'))},",
        f"  general_family = {_lua_string(theme.font('general'))},",
        "}",
    ]
    return "\n".join(lines) + "\n"


def emit(theme, out):
    for profile in list_profiles():
        scheme = theme if profile == theme.profile else Theme.load(profile)
        out.write(path(COLORS_DIR, f"{profile}.lua"), _scheme(scheme))
    out.write(path(TYPES_FILE), _types(theme))
    out.write(path(COLORS_DIR, "profiles.lua"), _color_profiles(theme))
    for role in font_roles():
        out.write(path(FONTS_DIR, f"{role}.lua"), _font(theme, role))
    out.write(path(FONTS_DIR, "fonts.lua"), _fonts(theme))
