import re

ANSI_KEYS = ("black", "red", "green", "yellow", "blue", "magenta", "cyan", "white")
UI_KEYS = ("background", "primary", "accent", "surface", "foreground")
TOP_LEVEL_KEYS = ("name", "dark", "ui", "ansi")

HEX_COLOR = re.compile(r"#[0-9a-fA-F]{6}")


def _keys(table, expected, where, problems):
    missing = sorted(set(expected) - set(table))
    extra = sorted(set(table) - set(expected))
    for name in missing:
        problems.append(f"{where}: missing '{name}'")
    for name in extra:
        problems.append(f"{where}: unknown '{name}'")


def _color_table(table, expected, where, problems):
    if not isinstance(table, dict):
        problems.append(f"{where}: must be a table")
        return {}
    _keys(table, expected, where, problems)
    colors = {}
    for name in expected:
        value = table.get(name)
        if not isinstance(value, str) or not HEX_COLOR.fullmatch(value):
            problems.append(f"{where}.{name}: must be a six-digit hex color")
        else:
            colors[name] = value.lower()
    return colors


def parse_profile(raw, source):
    problems = []
    if not isinstance(raw, dict):
        raise SystemExit(f"dotfile theme: {source}: profile must be a TOML table")
    _keys(raw, TOP_LEVEL_KEYS, source, problems)

    name = raw.get("name")
    if not isinstance(name, str) or not name.strip():
        problems.append(f"{source}.name: must be a non-empty string")
    dark = raw.get("dark")
    if not isinstance(dark, bool):
        problems.append(f"{source}.dark: must be true or false")

    ui = _color_table(raw.get("ui"), UI_KEYS, f"{source}.[ui]", problems)
    ansi_raw = raw.get("ansi")
    if not isinstance(ansi_raw, dict):
        problems.append(f"{source}.[ansi]: must be a table")
        ansi_raw = {}
    else:
        _keys(ansi_raw, ("normal", "bright"), f"{source}.[ansi]", problems)
    normal = _color_table(ansi_raw.get("normal"), ANSI_KEYS, f"{source}.[ansi.normal]", problems)
    bright = _color_table(ansi_raw.get("bright"), ANSI_KEYS, f"{source}.[ansi.bright]", problems)

    if problems:
        listed = "\n".join(f"  {problem}" for problem in problems)
        raise SystemExit(f"dotfile theme: profile is not usable:\n{listed}")
    return {
        "name": name.strip(),
        "dark": dark,
        "ui": ui,
        "ansi": {"normal": normal, "bright": bright},
    }


def primitives(profile):
    values = {f"ui.{name}": profile["ui"][name] for name in UI_KEYS}
    for group in ("normal", "bright"):
        for name in ANSI_KEYS:
            values[f"ansi.{group}.{name}"] = profile["ansi"][group][name]
    return values
