from tools.theme import contrast, oklab, tmux, yazi
from tools.theme.model import COLOR_TABLES, FONT_SIZES, Theme, list_profiles, load_map, path


def _leaves(table):
    for key, value in table.items():
        if isinstance(value, dict):
            yield from _leaves(value)
        elif isinstance(value, str):
            yield key, value


def _check_expressions(theme, problems):
    problems.extend(tmux.expression_problems(theme))
    for name in COLOR_TABLES:
        for key, expression in _leaves(theme.data.get(name, {})):
            try:
                theme.hex(expression)
            except SystemExit as error:
                problems.append(f"[{name}] {key} -> {error}")

    for key, expression in load_map("gtk")["colors"].items():
        try:
            theme.mapped_color("kde", expression)
        except SystemExit as error:
            problems.append(f"maps/gtk.toml {key} -> {error}")

    for key, expression in load_map("nvim")["colors"].items():
        try:
            theme.hex(expression)
        except SystemExit as error:
            problems.append(f"maps/nvim.toml {key} -> {error}")

    obsidian = load_map("obsidian")
    expressions = [("[derived] source", obsidian["derived"]["source"])]
    for key, value in obsidian["variables"].items():
        if isinstance(value, str):
            expressions.append((key, value))
        elif "rgb" in value:
            expressions.append((key, value["rgb"]))
        elif "color" in value:
            expressions.append((key, value["color"]))
    for key, expression in expressions:
        try:
            theme.resolved(expression)
        except SystemExit as error:
            problems.append(f"maps/obsidian.toml {key} -> {error}")

    template = _yazi_template()
    problems.extend(yazi.schema_problems(template))
    for state, style in yazi.styles(template):
        for channel in ("fg", "bg"):
            expression = style.get(channel)
            if not expression or expression == "reset":
                continue
            try:
                theme.hex(expression)
            except SystemExit as error:
                problems.append(f"maps/yazi.toml {state}.{channel} -> {error}")


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


def _check_lightness(theme, problems):
    background = oklab.relative_luminance(theme.hex("ui.background"))
    foreground = oklab.relative_luminance(theme.hex("ui.foreground"))
    if theme.dark and background >= foreground:
        problems.append("dark=true but ui.background is not darker than ui.foreground")
    if not theme.dark and background <= foreground:
        problems.append("dark=false but ui.background is not lighter than ui.foreground")


def _yazi_template():
    with open(path("theme/maps/yazi.toml"), encoding="utf-8") as handle:
        return handle.read()


def _check_contrast(theme, problems):
    for pair in contrast.required_pairs(theme, _yazi_template()):
        if not pair.passes:
            problems.append(
                f"{pair.area}.{pair.state}: {pair.foreground} on {pair.background} is "
                f"{pair.ratio:.2f}:1, under {pair.floor:.1f}:1"
            )


def validate(theme):
    problems = []
    _check_expressions(theme, problems)
    _check_fonts(theme, problems)
    _check_lightness(theme, problems)
    _check_contrast(theme, problems)
    if problems:
        listed = "\n".join(f"  {problem}" for problem in problems)
        raise SystemExit(f"dotfile theme: profile '{theme.profile}' is not usable:\n{listed}")


def validate_all():
    themes = [Theme.load(name) for name in list_profiles()]
    names = {}
    problems = []
    for theme in themes:
        if theme.name in names:
            problems.append(
                f"duplicate display name {theme.name!r}: {names[theme.name]}, {theme.profile}"
            )
        names[theme.name] = theme.profile
        try:
            validate(theme)
        except SystemExit as error:
            problems.append(str(error))
    if problems:
        raise SystemExit("dotfile theme: profiles are not usable:\n" + "\n".join(problems))
    return themes
