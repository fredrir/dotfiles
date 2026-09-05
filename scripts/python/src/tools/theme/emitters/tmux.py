from tools.theme import tmux
from tools.theme.model import load_map, path

OUTPUT = "shared/tmux/theme.conf"


def render(theme):
    mapping = load_map("tmux")
    colors = tmux.colors(theme)
    lines = [f"# {theme.header}", f"set -g @theme_name '{theme.profile}'"]
    lines.extend(f"set -g @theme_{name} '{value}'" for name, value in colors.items())
    fzf_colors = ",".join(f"{name}:{colors[role]}" for name, role in mapping["fzf"].items())
    lines.extend([f"set -g @theme_fzf_colors '{fzf_colors}'", ""])
    lines.extend(f"set -g @fingers-{name}-style '{style}'" for name, style in tmux.fingers_styles(theme).items())
    for name, style in mapping["styles"].items():
        value = f"fg={colors[style['fg']]},bg={colors[style['bg']]}"
        if style.get("attrs"):
            value += f",{style['attrs']}"
        lines.append(f"set -g {name} '{value}'")
    lines.extend(
        [
            "",
            f"set -g display-panes-colour '{colors['muted']}'",
            f"set -g display-panes-active-colour '{colors['primary']}'",
            f"set -g clock-mode-colour '{colors['primary']}'",
            f"set -ag message-style ',fill={colors['surface']}'",
            f"set -ag message-command-style ',fill={colors['active_bg']}'",
        ]
    )
    return "\n".join(lines) + "\n"


def emit(theme, out):
    out.write(path(OUTPUT), render(theme))
