import re
from dataclasses import dataclass

from rich.console import Console, Group
from rich.table import Table
from rich.text import Text

from tools.core.console import colors_enabled
from tools.theme.model import Theme
from tools.utils.sysinfo.branding import header_illustration, illustration, resolve_brand
from tools.utils.sysinfo.health import health_summary
from tools.utils.sysinfo.identity import display_hostname, display_username
from tools.utils.sysinfo.models import Component, HealthIssue, RenderOptions, SystemView
from tools.utils.sysinfo.typography import block_text


@dataclass(frozen=True)
class Colors:
    text: str
    subtext: str
    overlay: str
    system: str
    hardware: str
    desktop: str
    green: str
    yellow: str
    red: str


def load_colors():
    theme = Theme.load()
    return Colors(
        text=theme.hex("text"),
        subtext=theme.hex("subtext"),
        overlay=theme.hex("overlay"),
        system=theme.role("section_system"),
        hardware=theme.role("section_hardware"),
        desktop=theme.role("section_desktop"),
        green=theme.hex("green"),
        yellow=theme.hex("yellow"),
        red=theme.hex("red"),
    )


def heading(label, color):
    value = Text()
    value.append(label.upper(), style=f"bold {color}")
    return value


def compact_software_label(label):
    return re.sub(r"\s+\d+(?:\.\d+)+(?:[-+._a-z0-9]*)?$", "", label, flags=re.IGNORECASE)


def header_environment(view, colors):
    line = Text()
    badges = [badge for badge in view.software if badge.kind in {"desktop", "wm", "session"}]
    for index, badge in enumerate(badges):
        if index:
            line.append("   ", style=colors.overlay)
        brand = resolve_brand(badge.kind, badge.vendor, badge.label, *badge.identifiers)
        line.append(compact_software_label(badge.label).upper(), style=f"bold {brand.accent}")
    return line


def identity_header(view, issues, colors, width):
    brand = resolve_brand(
        view.platform.kind,
        view.platform.vendor,
        view.platform.label,
        *view.platform.identifiers,
    )
    username = display_username().upper()
    hostname = display_hostname().upper()
    identity = Table.grid(expand=False)
    identity.add_column()
    owner = Text()
    owner.append(username, style=f"bold {colors.text}")
    owner.append("   ", style=colors.overlay)
    owner.append(view.machine_type, style=f"bold {colors.overlay}")
    identity.add_row(owner)
    identity.add_row(Text())
    hostname_art = block_text(hostname)
    for line in hostname_art:
        identity.add_row(Text(line, style=f"bold {brand.accent}"))
    identity.add_row(Text(hostname, style=f"bold {colors.subtext}"))
    identity.add_row(Text())
    platform = Text()
    if brand.mark:
        platform.append(f"{brand.mark}  ", style=f"bold {brand.accent}")
    platform.append(
        compact_software_label(view.platform.label).upper(),
        style=f"bold {brand.accent}",
    )
    identity.add_row(platform)
    environment = header_environment(view, colors)
    if environment:
        identity.add_row(environment)
    summary = health_summary(issues)
    if summary:
        errors = any(issue.severity == "error" for issue in issues)
        identity.add_row(Text(summary, style=f"bold {colors.red if errors else colors.yellow}"))

    art = header_illustration(brand)
    art_width = max(len(line) for line in art)
    identity_width = max(len(line) for line in hostname_art)
    if width < 76 or art_width + identity_width + 4 > width:
        return identity
    grid = Table.grid(expand=False, padding=(0, 4))
    grid.add_column(no_wrap=True)
    grid.add_column()
    grid.add_row(Text("\n".join(art), style=f"bold {brand.accent}"), identity)
    return grid


def component_card(component: Component, width, colors):
    brand = resolve_brand(
        component.kind,
        component.vendor,
        component.model,
        *component.identifiers,
    )
    body = Table.grid(expand=False)
    body.add_column()
    title = Text()
    generic = brand.key == component.kind
    if brand.mark and not generic:
        title.append(f"{brand.mark}  ", style=f"bold {brand.accent}")
    if generic:
        title.append(component.label, style=f"bold {brand.accent}")
    else:
        title.append(brand.name, style=f"bold {brand.accent}")
        if component.label.casefold() != brand.name.casefold():
            title.append(f"  {component.label}", style=colors.overlay)
    body.add_row(title)
    body.add_row(Text(component.model, style=f"bold {colors.text}"))
    if component.facts:
        details = Table.grid(expand=False, padding=(0, 2))
        details.add_column(width=12, no_wrap=True, style=colors.subtext)
        details.add_column(style=colors.text)
        for item in component.facts:
            details.add_row(item.label, item.value)
        body.add_row(details)
    art = illustration(brand, component.art_kind or component.kind)
    if width < 46 or not art:
        return body
    card = Table.grid(expand=False, padding=(0, 2))
    card.add_column(width=max(len(line) for line in art), no_wrap=True)
    card.add_column()
    card.add_row(Text("\n".join(art), style=f"bold {brand.accent}"), body)
    return card


def hardware_grid(view, width, colors):
    card_width = width if width < 94 else width // 2
    cards = [component_card(component, card_width, colors) for component in view.components]
    if width < 94:
        blocks = []
        for index, card in enumerate(cards):
            if index:
                blocks.append(Text())
            blocks.append(card)
        return Group(*blocks)
    grid = Table.grid(expand=True, padding=(0, 4))
    grid.add_column(ratio=1)
    grid.add_column(ratio=1)
    for index in range(0, len(cards), 2):
        left = Group(cards[index], Text())
        right = Group(cards[index + 1], Text()) if index + 1 < len(cards) else Text()
        grid.add_row(left, right)
    return grid


def software_strip(view, options, colors, width):
    values = []
    for badge in view.software:
        brand = resolve_brand(badge.kind, badge.vendor, badge.label, *badge.identifiers)
        value = Text()
        value.append(f"{brand.mark} ", style=f"bold {brand.accent}")
        label = badge.label if options.full else compact_software_label(badge.label)
        value.append(label, style=colors.subtext)
        values.append(value)
    if not values:
        return None
    if width < 70:
        return Group(*values)
    line = Text()
    for index, value in enumerate(values):
        if index:
            line.append("    ")
        line.append_text(value)
    return line


def system_details(view, colors):
    if not view.system_facts:
        return None
    table = Table.grid(expand=False, padding=(0, 2))
    table.add_column(width=20, no_wrap=True, style=colors.subtext)
    table.add_column(style=colors.text)
    for item in view.system_facts:
        table.add_row(item.label, item.value)
    return table


def health_details(issues, colors):
    blocks = []
    for index, issue in enumerate(issues):
        if index:
            blocks.append(Text())
        color = colors.red if issue.severity == "error" else colors.yellow
        title = Text()
        title.append(issue.severity.upper(), style=f"bold {color}")
        title.append(f"  {issue.title}", style=f"bold {colors.text}")
        blocks.append(title)
        if issue.detail:
            blocks.append(Text(issue.detail, style=colors.subtext))
        if issue.action:
            action = Text()
            action.append("Action  ", style=f"bold {colors.desktop}")
            action.append(issue.action, style=colors.text)
            blocks.append(action)
    return Group(*blocks)


def render_pretty(
    view: SystemView,
    issues: tuple[HealthIssue, ...],
    options: RenderOptions,
    console=None,
):
    colors = load_colors()
    console = console or Console(
        highlight=False,
        soft_wrap=False,
        no_color=not colors_enabled(),
    )
    width = max(36, min(console.width, 132))
    blocks = [identity_header(view, issues, colors, width), Text()]
    blocks.extend([heading("Hardware", colors.hardware), Text()])
    blocks.extend([hardware_grid(view, width, colors), Text()])
    if options.full:
        software = software_strip(view, options, colors, width)
        if software:
            blocks.extend([heading("Software", colors.desktop), software])
        details = system_details(view, colors)
        if details:
            blocks.extend([Text(), heading("System", colors.system), Text(), details])
    if options.health and issues:
        blocks.extend(
            [Text(), heading("Health", colors.yellow), Text(), health_details(issues, colors)]
        )
    console.print(Group(*blocks))
