from tools.theme.model import load_map


def colors(theme):
    return {name: theme.hex(expression) for name, expression in load_map("tmux")["colors"].items()}


def color_pairs():
    mapping = load_map("tmux")
    for state, style in mapping["styles"].items():
        yield state, style["fg"], style["bg"], style.get("floor", 4.5)

    for state, color in mapping["fzf"].items():
        if state in {"bg", "bg+", "preview-bg", "gutter"}:
            continue
        background = "active_bg" if state.endswith("+") else "bg"
        yield f"fzf.{state}", color, background, 3.0 if state == "border" else 4.5

    for state, foreground, background in (
        ("status.active", "active_fg", "active_bg"),
        ("status.muted", "muted", "bg"),
        ("status.success", "success", "bg"),
        ("status.warning", "warning", "bg"),
        ("status.error", "error", "bg"),
        ("pane.label", "primary", "bg"),
        ("pane.number", "muted", "bg"),
    ):
        yield state, foreground, background, 4.5


def expression_problems(theme):
    mapping = load_map("tmux")
    problems = []
    for key, expression in mapping["colors"].items():
        try:
            theme.hex(expression)
        except SystemExit as error:
            problems.append(f"maps/tmux.toml colors.{key} -> {error}")
    for state, foreground, background, _floor in color_pairs():
        for name in (foreground, background):
            if name not in mapping["colors"]:
                problems.append(f"maps/tmux.toml {state}: unknown color '{name}'")
    return problems
