from tools.theme.model import path
from tools.theme.render import replace_between

OUTPUT = "linux/common/quicklaunch/config.toml"

KEYS = (
    ("accent", "accent"),
    ("background", "view_bg"),
    ("text", "foreground"),
    ("muted", "inactive"),
    ("selection", "selection_bg"),
)


def emit(theme, out):
    width = max(len(key) for key, _ in KEYS)
    lines = [f"# {theme.header}"]
    for key, role in KEYS:
        lines.append(f'{key.ljust(width)} = "{theme.kde(role)}"')
    out.edit(path(OUTPUT), lambda text: replace_between(text, "quicklaunch", lines))
