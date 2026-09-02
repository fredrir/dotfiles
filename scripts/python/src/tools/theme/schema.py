import re
from dataclasses import dataclass
from types import MappingProxyType

UI_KEYS = ("background", "primary", "accent", "surface", "foreground")
ANSI_KEYS = ("black", "red", "green", "yellow", "blue", "magenta", "cyan", "white")
ROOT_KEYS = ("name", "dark", "ui", "ansi")
ANSI_TABLES = ("normal", "bright")

HEX = re.compile(r"#[0-9a-f]{6}")


@dataclass(frozen=True)
class Profile:
    name: str
    dark: bool
    primitives: MappingProxyType


def _keys(table, expected, where, problems):
    if not isinstance(table, dict):
        problems.append(f"{where} must be a table")
        return
    for name in expected:
        if name not in table:
            problems.append(f"{where}.{name} is missing")
    for name in table:
        if name not in expected:
            problems.append(f"{where}.{name} is not allowed")


def _color(table, name, where, problems):
    if name not in table:
        return None
    value = table.get(name)
    key = f"{where}.{name}"
    if not isinstance(value, str):
        problems.append(f"{key} must be a #rrggbb string")
        return None
    if not HEX.fullmatch(value):
        problems.append(f"{key} must be #rrggbb: {value!r}")
        return None
    return value


def parse_profile(data, source):
    problems = []
    if not isinstance(data, dict):
        raise SystemExit(f"dotfile theme: {source} must contain a TOML table")

    _keys(data, ROOT_KEYS, "profile", problems)

    name = data.get("name")
    if not isinstance(name, str) or not name.strip():
        problems.append("profile.name must be a non-empty string")

    dark = data.get("dark")
    if not isinstance(dark, bool):
        problems.append("profile.dark must be true or false")

    ui = data.get("ui")
    _keys(ui, UI_KEYS, "ui", problems)

    ansi = data.get("ansi")
    _keys(ansi, ANSI_TABLES, "ansi", problems)
    if isinstance(ansi, dict):
        for table_name in ANSI_TABLES:
            _keys(ansi.get(table_name), ANSI_KEYS, f"ansi.{table_name}", problems)

    primitives = {}
    if isinstance(ui, dict):
        for key in UI_KEYS:
            value = _color(ui, key, "ui", problems)
            if value is not None:
                primitives[f"ui.{key}"] = value
    if isinstance(ansi, dict):
        for table_name in ANSI_TABLES:
            table = ansi.get(table_name)
            if not isinstance(table, dict):
                continue
            for key in ANSI_KEYS:
                value = _color(table, key, f"ansi.{table_name}", problems)
                if value is not None:
                    primitives[f"ansi.{table_name}.{key}"] = value

    if problems:
        listed = "\n".join(f"  {problem}" for problem in problems)
        raise SystemExit(f"dotfile theme: profile '{source}' is not usable:\n{listed}")

    return Profile(name=name.strip(), dark=dark, primitives=MappingProxyType(primitives))
