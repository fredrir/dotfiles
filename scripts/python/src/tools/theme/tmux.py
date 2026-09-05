from tools.theme.model import load_map


def indexed_rgb(index):
    if index >= 232:
        return (8 + 10 * (index - 232),) * 3
    ramp = (0, 95, 135, 175, 215, 255)
    index -= 16
    return ramp[index // 36], ramp[index // 6 % 6], ramp[index % 6]


def indexed_color(value, background):
    def rgb(color):
        return tuple(int(color[i : i + 2], 16) for i in (1, 3, 5))

    def luminance(color):
        linear = [x / 3294.6 if x <= 10 else ((x / 255 + 0.055) / 1.055) ** 2.4 for x in color]
        return sum(x * weight for x, weight in zip(linear, (0.2126, 0.7152, 0.0722)))

    wanted, bg = rgb(value), luminance(rgb(background))
    candidates = [(index, indexed_rgb(index)) for index in range(16, 256)]
    visible = [
        (index, color)
        for index, color in candidates
        if (max(luminance(color), bg) + 0.05) / (min(luminance(color), bg) + 0.05) >= 4.5
    ]
    return min(visible, key=lambda item: sum((a - b) ** 2 for a, b in zip(item[1], wanted)))[0]


def fingers_styles(theme):
    palette = colors(theme)
    return {
        style: f"fg=colour{indexed_color(palette[role], palette['bg'])}{attributes}"
        for style, role, attributes in (
            ("hint", "primary", ",bold"),
            ("highlight", "fg", ""),
            ("backdrop", "muted", ""),
            ("selected-hint", "success", ",bold"),
            ("selected-highlight", "success", ""),
        )
    }


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
