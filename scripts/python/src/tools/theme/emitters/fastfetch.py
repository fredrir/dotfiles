import re

from tools.theme.model import path
from tools.theme.render import replace_between

SECTIONS = ["section_system", "section_hardware", "section_desktop", "section_network"]

CONFIGS = [
    "shared/fastfetch/config.jsonc",
    "linux/arch/fastfetch/config.jsonc",
    "linux/ubuntu/fastfetch/config.jsonc",
    "macos/fastfetch/config.jsonc",
]

LOGOS = [
    "linux/arch/fastfetch/arch.txt",
    "linux/ubuntu/fastfetch/ubuntu.txt",
    "macos/fastfetch/apple.txt",
]


def emit_config(theme, out):
    lines = []
    for role in SECTIONS:
        value = theme.role(role)
        lines.append(
            f'"{theme.truecolor(value, bold=True)}", // {theme.data["roles"][role]} {value}'
        )
    separator = theme.role("separator")
    lines.append(
        f'"{theme.truecolor(separator)}" // {theme.data["roles"]["separator"]} {separator}'
    )

    def transform(text):
        text = replace_between(text, "constants", lines)
        updated = text.split("\n")
        for index, line in enumerate(updated):
            if "theme:separator" in line:
                updated[index] = re.sub(r"#[0-9a-fA-F]{6}", separator, line)
        return "\n".join(updated)

    for config in CONFIGS:
        out.edit(path(config), transform)


def emit_logo(theme, out):
    stops = [theme.rgb(theme.role(role)) for role in SECTIONS]
    segments = len(stops) - 1

    def lerp(start, end, ratio):
        return tuple(round(start[i] + (end[i] - start[i]) * ratio) for i in range(3))

    for logo in LOGOS:
        target = path(logo)
        with open(target, encoding="utf-8") as handle:
            raw = handle.read().split("\n")
        trailing_newline = bool(raw) and raw[-1] == ""
        if trailing_newline:
            raw = raw[:-1]

        art = [re.sub(r"\x1b\[[0-9;]*m", "", line) for line in raw]
        count = len(art)

        body = []
        for index, line in enumerate(art):
            position = (index / (count - 1)) * segments if count > 1 else 0
            segment = min(int(position), segments - 1)
            red, green, blue = lerp(stops[segment], stops[segment + 1], position - segment)
            body.append(f"\x1b[1;38;2;{red};{green};{blue}m{line}")
        content = "\n".join(body) + "\x1b[0m"
        if trailing_newline:
            content += "\n"
        out.write(target, content)
