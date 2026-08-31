from rich.cells import cell_len
from rich.console import Console, Group
from rich.padding import Padding
from rich.table import Table
from rich.text import Text

from tools.core.console import colors_enabled, stdout
from tools.core.typography import block_text
from tools.theme.emitters import ANSI_NORMAL
from tools.theme.model import Theme

CARD_WIDTH = 56
CARD_MIN = 46
SAMPLE_FILES = (
    ("scripts", "di"),
    ("theme", "di"),
    ("starship.toml", "*.toml"),
    ("setup.sh", "*.sh"),
)


def width():
    return max(CARD_MIN, min(stdout.width, 132))


def to_lines(renderable, columns):
    console = Console(
        width=columns,
        force_terminal=True,
        color_system="truecolor",
        highlight=False,
        markup=False,
        soft_wrap=True,
        no_color=not colors_enabled(),
    )
    with console.capture() as capture:
        console.print(renderable)
    return capture.get().splitlines()


def terminal(theme, key):
    return theme.hex(theme.data["terminal"][key])


def ansi(theme, key):
    return theme.hex(key)


def eza(theme, key):
    table = theme.data["eza"]
    return theme.hex(table.get(key, table["fi"]))


def heading(label, color):
    return Text(label.upper(), style=f"bold {color}")


def chips(theme, names, gap=" "):
    line = Text()
    for index, name in enumerate(names):
        if index:
            line.append(gap)
        line.append("███", style=ansi(theme, name))
    return line


def _painted(background, *segments):
    line = Text()
    for content, color in segments:
        line.append(content, style=f"{color} on {background}" if color else f"on {background}")
    return line


def _fill(line, columns, background):
    gap = columns - cell_len(line.plain)
    if gap > 0:
        line.append(" " * gap, style=f"on {background}")
    return line


def _blank(columns, background):
    return _fill(Text(), columns, background)


def _title_row(theme, columns, background):
    name = _painted(
        background,
        ("  ", None),
        (theme.name, f"bold {terminal(theme, 'foreground')}"),
    )
    tag = _painted(
        background,
        (theme.profile, theme.role("separator")),
        ("  ", None),
    )
    gap = columns - cell_len(name.plain) - cell_len(tag.plain)
    name.append(" " * max(gap, 1), style=f"on {background}")
    name.append_text(tag)
    return name


def _chip_row(theme, columns, background):
    row = _painted(background, ("  ", None))
    for index, name in enumerate(ANSI_NORMAL):
        if index:
            row.append(" ", style=f"on {background}")
        row.append("███", style=f"{ansi(theme, name)} on {background}")
    return _fill(row, columns, background)


def _prompt_row(theme, columns, background):
    row = _painted(
        background,
        ("  ", None),
        ("~/dotfiles", theme.role("prompt_dir")),
        ("   ", None),
        ("main", theme.role("prompt_git")),
        ("   ", None),
        ("3.13", theme.role("prompt_python")),
        ("   ", None),
        ("1.2s", theme.role("prompt_duration")),
    )
    return _fill(row, columns, background)


def _command_row(theme, columns, background):
    row = _painted(
        background,
        ("  ", None),
        ("❯ ", theme.role("prompt_char")),
        ("eza", terminal(theme, "foreground")),
        ("█", terminal(theme, "cursor")),
    )
    return _fill(row, columns, background)


def _listing_row(theme, columns, background):
    row = _painted(background, ("  ", None))
    for index, (name, key) in enumerate(SAMPLE_FILES):
        if index:
            row.append("  ", style=f"on {background}")
        row.append(name, style=f"{eza(theme, key)} on {background}")
    return _fill(row, columns, background)


def _pills(theme, background):
    tabs = theme.data["terminal"].get("tabs", {})
    pills = [
        (
            "selection",
            terminal(theme, "selection_foreground"),
            terminal(theme, "selection_background"),
        ),
        ("accent", background, theme.kde("accent")),
    ]
    if "active_background" in tabs:
        pills.append(
            ("tab", theme.hex(tabs["active_foreground"]), theme.hex(tabs["active_background"]))
        )
    return pills


def _pill_row(theme, columns, background):
    row = _painted(background, ("  ", None))
    for index, (label, foreground, fill) in enumerate(_pills(theme, background)):
        if index:
            row.append("  ", style=f"on {background}")
        row.append(f" {label} ", style=f"{foreground} on {fill}")
    return _fill(row, columns, background)


def profile_card(theme, columns=CARD_WIDTH):
    if not colors_enabled():
        return None
    columns = max(columns, CARD_MIN)
    background = terminal(theme, "background")
    return Group(
        _blank(columns, background),
        _title_row(theme, columns, background),
        _chip_row(theme, columns, background),
        _blank(columns, background),
        _prompt_row(theme, columns, background),
        _command_row(theme, columns, background),
        _listing_row(theme, columns, background),
        _blank(columns, background),
        _pill_row(theme, columns, background),
        _blank(columns, background),
    )


def card_lines(theme, columns=CARD_WIDTH):
    card = profile_card(theme, columns)
    if card is None:
        return []
    return ["  " + line for line in to_lines(card, columns + 4)]


def _cells(items, columns, padding=4):
    table = Table.grid(padding=(0, padding))
    for _ in range(columns):
        table.add_column()
    rows = -(-len(items) // columns)
    for row in range(rows):
        picked = [row + rows * column for column in range(columns)]
        cells = [items[index] for index in picked if index < len(items)]
        table.add_row(*cells, *[Text()] * (columns - len(cells)))
    return table


def _entry(color, label, label_width, note):
    line = Text()
    line.append("███ ", style=color)
    line.append(label.ljust(label_width))
    line.append("  ")
    line.append(note, style="dim")
    return line


def _grid(cells, columns, most=3, padding=4):
    widest = max(cell_len(cell.plain) for cell in cells)
    across = max(1, min(most, (columns + padding) // (widest + padding)))
    return _cells(cells, across, padding)


def palette_grid(theme, columns):
    names = list(theme.palette)
    label_width = max(len(name) for name in names)
    cells = [_entry(theme.hex(name), name, label_width, theme.hex(name)) for name in names]
    return _grid(cells, columns)


def roles_grid(theme, columns):
    roles = theme.data.get("roles", {})
    if not roles:
        return None
    label_width = max(len(name) for name in roles)
    cells = [_entry(theme.hex(color), name, label_width, color) for name, color in roles.items()]
    return _grid(cells, columns)


def _facts(theme):
    table = Table.grid(padding=(0, 3))
    table.add_column(no_wrap=True, style="dim")
    table.add_column()
    table.add_row("family", theme.name)
    table.add_row("mode", "dark" if theme.dark else "light")
    if theme.icons:
        table.add_row("icons", theme.icons)
    flavour = theme.data.get("nvim", {}).get("flavour", "")
    if flavour:
        table.add_row("nvim", flavour)
    table.add_row("fonts", f"{theme.font('general')}  ·  {theme.font('nerd')}")
    sizes = "  ·  ".join(f"{name} {theme.size(name)}" for name in sorted(theme.sizes))
    table.add_row("sizes", sizes)
    table.add_row("source", f"theme/profiles/{theme.profile}.toml")
    return table


def profile_header(theme, columns):
    accent = theme.role("section_desktop")
    art = Text("\n".join(block_text(theme.profile.upper())), style=f"bold {accent}")
    facts = _facts(theme)
    if columns < 76:
        return Group(art, Text(), facts)
    grid = Table.grid(padding=(0, 5))
    grid.add_column(no_wrap=True)
    grid.add_column()
    grid.add_row(art, facts)
    return grid


def render_show(theme, scopes, count, columns=None):
    columns = (columns or width()) - 2
    blocks = [Text(), profile_header(theme, columns), Text()]
    card = profile_card(theme, min(columns, CARD_WIDTH))
    if card is not None:
        blocks.extend([card, Text()])
    blocks.extend([heading("palette", theme.role("section_hardware")), Text()])
    blocks.extend([palette_grid(theme, columns), Text()])
    roles = roles_grid(theme, columns)
    if roles is not None:
        blocks.extend([heading("roles", theme.role("section_system")), Text(), roles, Text()])
    blocks.append(heading("stamped into", theme.role("section_network")))
    blocks.append(Text())
    if scopes:
        lines = _packed(scopes, columns, 0)
        lines[-1].append(f"      {count} {'file' if count == 1 else 'files'}", style="dim")
        blocks.extend(lines)
    else:
        blocks.append(Text("no group is assigned to this profile", style="dim"))
    blocks.append(Text())
    stdout.print(Padding(Group(*blocks), (0, 0, 0, 2)))


def _headline(theme, scopes, count):
    assigned = bool(scopes)
    bullet = theme.role("section_desktop") if assigned else "dim"
    title = Text()
    title.append("  ")
    title.append("●" if assigned else "○", style=bullet)
    title.append("  ")
    title.append(theme.profile, style="bold" if assigned else "dim")
    title.append("   ")
    title.append(theme.name, style="" if assigned else "dim")
    title.append("   ")
    title.append("dark" if theme.dark else "light", style="dim")
    meta = f"{count} {'file' if count == 1 else 'files'}" if assigned else "unassigned"
    grid = Table.grid(expand=True)
    grid.add_column(ratio=1)
    grid.add_column(justify="right", no_wrap=True)
    grid.add_row(title, Text(meta + "  ", style="dim"))
    return grid


def _packed(values, columns, indent, separator="  ·  "):
    room = max(columns - indent - 2, 20)
    lines = []
    current = ""
    for value in values:
        candidate = f"{current}{separator}{value}" if current else value
        if current and cell_len(candidate) > room:
            lines.append(current)
            current = value
        else:
            current = candidate
    if current:
        lines.append(current)
    return [Text(" " * indent + line, style="dim") for line in lines]


def _status_block(theme, scopes, count, columns):
    swatch = Text("     ")
    swatch.append_text(chips(theme, ANSI_NORMAL))
    return [_headline(theme, scopes, count), swatch, *_packed(scopes, columns, 5), Text()]


def render_status(rows, changed, columns=None):
    columns = columns or width()
    blocks = [Text()]
    for theme, scopes, count in rows:
        blocks.extend(_status_block(theme, scopes, count, columns))
    footer = Text()
    if changed:
        count = f"{len(changed)} generated {'file' if len(changed) == 1 else 'files'}"
        footer.append("  ! ", style="bold yellow")
        footer.append(f"{count} would change")
        footer.append("      dotfile theme sync", style="dim")
    else:
        footer.append("  ✓ ", style="bold green")
        footer.append("every generated file matches its profile")
    blocks.extend([footer, Text()])
    stdout.print(Group(*blocks))


def render_changes(changed, dry):
    summary = Text()
    summary.append("  ")
    count = f"{len(changed)} {'file' if len(changed) == 1 else 'files'}"
    if dry:
        summary.append("! ", style="bold yellow")
        summary.append(f"{count} would change")
    else:
        summary.append("✓ ", style="bold green")
        summary.append(f"regenerated {count}")
    listed = Group(*[Text(f"      {target}", style="dim") for target in changed])
    stdout.print(Group(summary, listed))


def picker_preview(names, columns=None):
    columns = columns or min(width(), CARD_WIDTH)
    cache = {}

    def preview(index):
        if index not in cache:
            try:
                cache[index] = card_lines(Theme.load(names[index]), columns)
            except SystemExit:
                cache[index] = []
        return cache[index]

    return preview
