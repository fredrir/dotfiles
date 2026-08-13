from tools.theme.model import COLOR_TABLES, FONT_SIZES, load_map


def _leaves(table):
    for key, value in table.items():
        if isinstance(value, dict):
            yield from _leaves(value)
        elif isinstance(value, str):
            yield key, value


def _palette_references(theme):
    for name in COLOR_TABLES:
        for key, value in _leaves(theme.data.get(name, {})):
            yield f"[{name}] {key}", value

    for key, value in load_map("catppuccin")["colors"].items():
        yield f"maps/catppuccin.toml {key}", value

    obsidian = load_map("obsidian")
    yield "maps/obsidian.toml [derived] source", obsidian["derived"]["source"]
    for key, value in obsidian["variables"].items():
        if isinstance(value, str):
            yield f"maps/obsidian.toml {key}", value
        elif "rgb" in value:
            yield f"maps/obsidian.toml {key}", value["rgb"]
        elif "color" in value:
            yield f"maps/obsidian.toml {key}", value["color"]


def _check_palette_names(theme, problems):
    for where, name in _palette_references(theme):
        if name not in theme.palette:
            problems.append(f"{where} -> unknown palette color '{name}'")

    kde = theme.data.get("kde", {})
    for key, name in load_map("gtk")["colors"].items():
        if name in kde:
            if kde[name] not in theme.palette:
                problems.append(f"maps/gtk.toml {key} -> [kde] {name} -> unknown color '{kde[name]}'")
        elif name not in theme.palette:
            problems.append(f"maps/gtk.toml {key} -> unknown palette color or kde role '{name}'")


def _check_palette_shape(theme, problems):
    seen = {}
    for name, value in theme.palette.items():
        lowered = value.lower()
        if lowered in seen:
            problems.append(
                f"[palette] '{name}' and '{seen[lowered]}' are both {lowered};"
                " duplicate colors cannot survive a profile switch"
            )
        else:
            seen[lowered] = name

    for name in theme.data.get("kde", {}):
        if name in theme.palette:
            problems.append(
                f"[kde] '{name}' shadows the palette color of the same name,"
                " which silently changes what maps/gtk.toml resolves to"
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
    _check_palette_names(theme, problems)
    _check_palette_shape(theme, problems)
    _check_eza_categories(theme, problems)
    _check_fonts(theme, problems)
    if problems:
        listed = "\n".join(f"  {problem}" for problem in problems)
        raise SystemExit(f"dotfile theme: profile '{theme.profile}' is not usable:\n{listed}")
