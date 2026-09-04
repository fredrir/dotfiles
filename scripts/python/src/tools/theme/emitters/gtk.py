import re
import sys

from tools.theme.model import load_map, path
from tools.theme.render import set_ini_key

VERSIONS = ("gtk-3.0", "gtk-4.0")


def color_outputs():
    return [f"linux/common/gtk/{version}/colors.css" for version in VERSIONS]


def settings_outputs():
    return [f"linux/common/gtk/{version}/settings.ini" for version in VERSIONS]


def emit_colors(theme, out):
    mapping = load_map("gtk")["colors"]
    pattern = re.compile(r"(@define-color\s+)(\S+)(\s+)(#[0-9a-fA-F]{6,8})(;.*)$")

    def transform(text):
        lines = []
        for line in text.split("\n"):
            match = pattern.match(line)
            if match:
                variable = match.group(2)
                base = variable.removesuffix("_breeze")
                if base in mapping:
                    color = theme.mapped_color("kde", mapping[base])
                    line = f"{match.group(1)}{variable}{match.group(3)}{color}{match.group(5)}"
                else:
                    sys.stderr.write(f"dotfile theme: unmapped GTK color '{variable}'\n")
            lines.append(line)
        return "\n".join(lines)

    for target in color_outputs():
        out.edit(path(target), transform)


def emit_settings(theme, out):
    font = f"{theme.font('general')},  {theme.size('interface')}"
    prefer_dark = "true" if theme.dark else "false"

    for target in settings_outputs():

        def transform(text, where=target):
            text = set_ini_key(text, "Settings", "gtk-font-name", font, where)
            text = set_ini_key(
                text, "Settings", "gtk-application-prefer-dark-theme", prefer_dark, where
            )
            return set_ini_key(text, "Settings", "gtk-icon-theme-name", theme.icons, where)

        out.edit(path(target), transform)
