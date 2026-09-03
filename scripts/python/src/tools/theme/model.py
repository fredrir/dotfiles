import glob
import os
import tomllib

from tools.core.paths import repo_root
from tools.theme import derive, oklab
from tools.theme.schema import ANSI_KEYS, parse_profile, primitives
from tools.theme.semantic import resolve_semantics

ROOT = str(repo_root())
THEME_DIR = os.path.join(ROOT, "theme")
MAPS_DIR = os.path.join(THEME_DIR, "maps")
PROFILES_DIR = os.path.join(THEME_DIR, "profiles")

ROLES_FILE = os.path.join(THEME_DIR, "roles.toml")
FONTS_FILE = os.path.join(THEME_DIR, "fonts.toml")
COLOR_TABLES = ("roles", "terminal", "eza", "kde", "konsole")
FONT_SIZES = ("terminal", "interface")
ANSI = ANSI_KEYS


def path(*parts):
    return os.path.join(ROOT, *parts)


def load_toml(target):
    with open(target, "rb") as handle:
        return tomllib.load(handle)


def load_map(name):
    return load_toml(os.path.join(MAPS_DIR, f"{name}.toml"))


def profile_file(name):
    return os.path.join(PROFILES_DIR, f"{name}.toml")


def list_profiles():
    found = glob.glob(os.path.join(PROFILES_DIR, "*.toml"))
    return sorted(os.path.basename(target)[: -len(".toml")] for target in found)


def profile_palette(name):
    theme = Theme.load(name)
    names = ["background", "primary", "accent", "surface", "foreground"]
    names += [*ANSI, *(f"bright_{name}" for name in ANSI)]
    return {name: theme.hex(name) for name in names}


class Theme:
    def __init__(self, profile, profile_data, data, fonts):
        self.profile = profile
        self.profile_data = profile_data
        self.data = data
        self.primitives = primitives(profile_data)
        self.semantic = resolve_semantics(self.primitives)
        exported = ["background", "primary", "accent", "surface", "foreground"]
        exported += [*ANSI, *(f"bright_{name}" for name in ANSI)]
        self.palette = {name: self.semantic[name] for name in exported}
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
        profile_data = parse_profile(load_toml(target), os.path.relpath(target, ROOT))
        return cls(name, profile_data, load_toml(ROLES_FILE), load_toml(FONTS_FILE))

    @property
    def name(self):
        return self.profile_data["name"]

    @property
    def dark(self):
        return self.profile_data["dark"]

    @property
    def icons(self):
        return "Breeze Chameleon Dark" if self.dark else "breeze"

    @property
    def header(self):
        return f"Generated from theme/profiles/{self.profile}.toml"

    def hex(self, name):
        return self.resolved(name).hex

    def resolved(self, name):
        return derive.resolve(
            name,
            self._lookup,
            self.primitives["ui.background"],
            self.primitives["ui.foreground"],
        )

    def _lookup(self, name):
        if name in self.primitives:
            return self.primitives[name]
        if name in self.semantic:
            return self.semantic[name]
        if name in self.palette:
            return self.palette[name]
        raise SystemExit(f"unknown palette color: {name}")

    def role(self, name):
        try:
            return self.hex(self.data["roles"][name])
        except KeyError:
            raise SystemExit(f"unknown role: {name}")

    def kde(self, name):
        return self.app_color("kde", name)

    def konsole(self, name):
        return self.app_color("konsole", name)

    def app_color(self, application, name):
        try:
            expression = self.data[application][name]
        except KeyError:
            raise SystemExit(f"unknown {application} role: {name}")
        return self.hex(expression)

    def mapped_color(self, application, name):
        expression = self.data.get(application, {}).get(name, name)
        return self.hex(expression)

    def css(self, expression):
        resolved = self.resolved(expression)
        if resolved.alpha is None:
            return resolved.hex
        red, green, blue = self.rgb(resolved.hex)
        return f"rgba({red}, {green}, {blue}, {resolved.alpha:g})"

    def readable(self, expression, against, floor=4.5):
        return derive.resolve(
            f"readable({expression},{against},{floor:g})",
            self._lookup,
            self.primitives["ui.background"],
            self.primitives["ui.foreground"],
        ).hex

    def visible(self, expression, against, floor=3.0):
        return self.readable(expression, against, floor)

    def readable_many(self, expression, backgrounds, floor=4.5):
        seed = self.hex(expression)
        return oklab.ensure_contrast_many(seed, backgrounds, floor)

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
