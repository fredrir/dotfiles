from tools.theme.model import path
from tools.theme.render import replace_between

PROMPT_ROLES = ("prompt_python", "prompt_git", "prompt_dir", "prompt_duration", "prompt_char")
OUTPUT = "shared/starship/starship.toml"


def emit(theme, out):
    names = list(theme.palette.keys())
    lines = [f"# {theme.header}", "[palettes.theme]"]
    width = max(len(name) for name in names)
    for name in names:
        lines.append(f"{name.ljust(width)} = '{theme.hex(name)}'")
    lines.append("")
    width = max(len(role) for role in PROMPT_ROLES)
    for role in PROMPT_ROLES:
        lines.append(f"{role.ljust(width)} = '{theme.role(role)}'")
    out.edit(path(OUTPUT), lambda text: replace_between(text, "palette", lines))
