import copy
import glob
import os
import tomllib

from tools.core.paths import repo_root
from tools.theme import derive

ROOT = str(repo_root())
THEME_DIR = os.path.join(ROOT, "theme")
MAPS_DIR = os.path.join(THEME_DIR, "maps")
PROFILES_DIR = os.path.join(THEME_DIR, "profiles")

ROLES_FILE = os.path.join(THEME_DIR, "roles.toml")
FONTS_FILE = os.path.join(THEME_DIR, "fonts.toml")
RAMP_FILE = os.path.join(THEME_DIR, "ramp.toml")

COLOR_TABLES = ("roles", "terminal", "eza", "kde", "konsole")
FONT_SIZES = ("terminal", "interface")
ANSI = ("black", "red", "green", "yellow", "blue", "magenta", "cyan", "white")
ALIASES = ("cursor", "cursor_text", "selection_bg", "selection_fg")


def path(*parts):
    return os.path.join(ROOT, *parts)


def load_toml(target):
    with open(target, "rb") as handle:
        return tomllib.load(handle)


def load_map(name):
    return load_toml(os.path.join(MAPS_DIR, f"{name}.toml"))


def merge(base, override):
    result = copy.deepcopy(base)
    for key, value in override.items():
        if isinstance(value, dict) and isinstance(result.get(key), dict):
            result[key] = merge(result[key], value)
        else:
            result[key] = copy.deepcopy(value)
    return result


def profile_file(name):
    return os.path.join(PROFILES_DIR, f"{name}.toml")


def list_profiles():
    found = glob.glob(os.path.join(PROFILES_DIR, "*.toml"))
    return sorted(os.path.basename(target)[: -len(".toml")] for target in found)


def anchor_palette(colors):
    palette = {
        "bg": colors["primary"]["background"],
        "fg": colors["primary"]["foreground"],
        "cursor": colors["cursor"]["cursor"],
        "cursor_text": colors["cursor"]["text"],
        "selection_bg": colors["selection"]["background"],
        "selection_fg": colors["selection"]["text"],
    }
    for name in ANSI:
        palette[name] = colors["normal"][name]
        palette[f"bright_{name}"] = colors["bright"][name]
    return palette


def profile_palette(name):
    palette = anchor_palette(load_toml(profile_file(name))["colors"])
    return {key: value for key, value in palette.items() if key not in ALIASES}


class Theme:
    def __init__(self, profile, data, fonts):
        self.profile = profile
        self.data = data
        self.palette = anchor_palette(data["colors"])
        for name, expression in data.get("tokens", {}).items():
            self.palette[name] = self.hex(expression)
        self.fonts = fonts["fonts"]
        self.sizes = fonts.get("sizes", {})
        self.font_applications = fonts.get("applications", {})

    @classmethod
    def load(cls, profile=None):
        if profile is None:
            from tools.theme.profiles import default_profile

            profile = default_profile()
        name = profile
        target = profile_file(name)
        if not os.path.exists(target):
            available = ", ".join(list_profiles()) or "none"
            raise SystemExit(f"dotfile theme: unknown profile '{name}' (available: {available})")
        overrides = load_toml(target)
        data = merge(load_toml(ROLES_FILE), overrides)
        fonts = merge(
            load_toml(FONTS_FILE),
            {key: overrides[key] for key in ("fonts", "sizes", "applications") if key in overrides},
        )
        return cls(name, data, fonts)

    @property
    def name(self):
        return self.data["name"]

    @property
    def dark(self):
        return bool(self.data.get("dark", True))

    @property
    def icons(self):
        return self.data.get("icons", "")

    @property
    def header(self):
        return f"Generated from theme/profiles/{self.profile}.toml"

    def hex(self, name):
        return self.resolved(name).hex

    def resolved(self, name):
        return derive.resolve(name, self._lookup, self.palette["bg"], self.palette["fg"])

    def _lookup(self, name):
        try:
            return self.palette[name]
        except KeyError:
            raise SystemExit(f"unknown palette color: {name}")

    def role(self, name):
        try:
            return self.hex(self.data["roles"][name])
        except KeyError:
            raise SystemExit(f"unknown role: {name}")

    def kde(self, name):
        try:
            return self.hex(self.data["kde"][name])
        except KeyError:
            raise SystemExit(f"unknown kde role: {name}")

    def konsole(self, name):
        try:
            return self.hex(self.data["konsole"][name])
        except KeyError:
            raise SystemExit(f"unknown konsole role: {name}")

    def color(self, name):
        if name in self.data.get("kde", {}):
            return self.kde(name)
        return self.hex(name)

    def font(self, name):
        try:
            return self.fonts[name]
        except KeyError:
            raise SystemExit(f"unknown font role: {name}")

    def size(self, name):
        try:
            return self.sizes[name]
        except KeyError:
            raise SystemExit(f"unknown font size: {name}")

    def uses_fonts(self, application):
        enabled = self.font_applications.get(application, False)
        if not isinstance(enabled, bool):
            raise SystemExit(f"font application setting must be true or false: {application}")
        return enabled

    def rgb(self, value):
        digits = value.lstrip("#")
        return int(digits[0:2], 16), int(digits[2:4], 16), int(digits[4:6], 16)

    def rgb_csv(self, value):
        return "{},{},{}".format(*self.rgb(value))

    def truecolor(self, value, bold=False):
        red, green, blue = self.rgb(value)
        prefix = "1;" if bold else ""
        return f"\\u001b[{prefix}38;2;{red};{green};{blue}m"

    def ansi(self, value):
        return "38;2;{};{};{}".format(*self.rgb(value))
