import sys

from tools.theme import oklab
from tools.theme.model import ANSI, COLOR_TABLES, FONT_SIZES, RAMP_FILE, load_map, load_toml


def _leaves(table):
    for key, value in table.items():
        if isinstance(value, dict):
            yield from _leaves(value)
        elif isinstance(value, str):
            yield key, value


def _expressions(theme):
    for name in COLOR_TABLES:
        for key, value in _leaves(theme.data.get(name, {})):
            yield f"[{name}] {key}", value

    for key, value in load_map("gtk")["colors"].items():
        yield f"maps/gtk.toml {key}", value

    for key, value in load_map("nvim")["colors"].items():
        yield f"maps/nvim.toml {key}", value

    kde = load_map("kde")
    for group, names in kde["groups"].items():
        for name in names:
            yield f"maps/kde.toml {group}", name
    for table in ("foregrounds", "selection_foregrounds"):
        for key, value in kde[table].items():
            yield f"maps/kde.toml [{table}] {key}", value

    obsidian = load_map("obsidian")
    yield "maps/obsidian.toml [derived] source", obsidian["derived"]["source"]
    for key, value in obsidian["variables"].items():
        if isinstance(value, str):
            yield f"maps/obsidian.toml {key}", value
        elif "rgb" in value:
            yield f"maps/obsidian.toml {key}", value["rgb"]
        elif "color" in value:
            yield f"maps/obsidian.toml {key}", value["color"]

    yield from _yazi_expressions(load_map("yazi"))

    for key, value in load_map("catppuccin")["colors"].items():
        yield f"maps/catppuccin.toml {key}", value


def _yazi_expressions(value, where="maps/yazi.toml"):
    if isinstance(value, dict):
        for key, child in value.items():
            current = f"{where} {key}"
            if key in ("fg", "bg") and isinstance(child, str) and child != "reset":
                yield current, child
            else:
                yield from _yazi_expressions(child, current)
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from _yazi_expressions(child, f"{where}[{index}]")


def _check_expressions(theme, problems):
    for where, expression in _expressions(theme):
        try:
            theme.color(expression)
        except SystemExit as error:
            problems.append(f"{where} -> {error}")


def _accents():
    for name in ANSI:
        if name not in ("black", "white"):
            yield name
            yield f"bright_{name}"


def _check_contrast(theme, warnings):
    floors = load_toml(RAMP_FILE)["contrast"]
    background = theme.hex("background")
    checked = [
        ("foreground_on_background", "foreground"),
        ("muted_on_background", "muted"),
        ("primary_on_background", "primary"),
    ]
    checked += [("ansi_on_background", name) for name in _accents()]
    for floor, name in checked:
        try:
            value = theme.hex(name)
        except SystemExit:
            continue
        ratio = oklab.contrast_ratio(value, background)
        if ratio < floors[floor]:
            warnings.append(
                f"{name} {value} on background is {ratio:.2f}:1, under {floors[floor]}:1"
            )


def _check_eza_categories(theme, problems):
    categories = theme.data.get("eza", {}).get("categories", {})
    if not categories:
        return
    known = load_map("eza")["categories"]
    for name in categories:
        if name not in known:
            problems.append(f"[eza.categories] '{name}' is not a group in maps/eza.toml")


def _check_fonts(theme, problems):
    for role in ("general", "nerd"):
        family = theme.fonts.get(role)
        if not family:
            problems.append(f"[fonts] '{role}' is missing")
        elif "," in family:
            problems.append(f"[fonts] '{role}' must not contain a comma: {family!r}")

    for name in FONT_SIZES:
        if name not in theme.sizes:
            problems.append(f"[sizes] '{name}' is missing")
        elif not isinstance(theme.sizes[name], (int, float)):
            problems.append(f"[sizes] '{name}' must be a number")


def validate(theme):
    problems = []
    warnings = []
    _check_expressions(theme, problems)
    _check_eza_categories(theme, problems)
    _check_fonts(theme, problems)
    _check_contrast(theme, warnings)
    for warning in warnings:
        print(f"dotfile theme: {theme.profile}: {warning}", file=sys.stderr)
    if problems:
        listed = "\n".join(f"  {problem}" for problem in problems)
        raise SystemExit(f"dotfile theme: profile '{theme.profile}' is not usable:\n{listed}")
