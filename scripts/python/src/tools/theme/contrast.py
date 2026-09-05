from dataclasses import dataclass

from tools.theme import oklab, tmux, yazi
from tools.theme.model import ANSI, load_map


@dataclass(frozen=True)
class Pair:
    area: str
    state: str
    foreground: str
    background: str
    ratio: float
    floor: float
    kind: str
    enforced: bool = True

    @property
    def passes(self):
        return self.ratio + 1e-9 >= self.floor


def _pair(theme, area, state, foreground, background, floor=4.5, kind="text", enforced=True):
    fg = theme.hex(foreground)
    bg = theme.hex(background)
    return Pair(area, state, fg, bg, oklab.contrast_ratio(fg, bg), floor, kind, enforced)


def semantic_pairs(theme):
    specs = [
        ("text.canvas", "text_on_canvas", "canvas_bg", 4.5, "text"),
        ("text.panel", "text_on_panel", "panel_bg", 4.5, "text"),
        ("text.surface", "text_on_surface", "surface_fill", 4.5, "text"),
        ("text.muted.canvas", "muted_on_canvas", "canvas_bg", 4.5, "text"),
        ("text.muted.panel", "muted_on_panel", "panel_bg", 4.5, "text"),
        ("text.disabled.canvas", "disabled_on_canvas", "canvas_bg", 3.0, "inactive"),
        ("text.disabled.panel", "disabled_on_panel", "panel_bg", 3.0, "inactive"),
        ("primary.on_fill", "on_primary", "primary_fill", 4.5, "text"),
        ("primary.text.canvas", "primary_text_on_canvas", "canvas_bg", 4.5, "text"),
        ("primary.text.panel", "primary_text_on_panel", "panel_bg", 4.5, "text"),
        ("primary.focus", "focus_ring", "canvas_bg", 3.0, "graphic"),
        ("border.canvas", "border_on_canvas", "canvas_bg", 3.0, "graphic"),
        ("border.panel", "border_on_panel", "panel_bg", 3.0, "graphic"),
    ]
    for name in ("info", "success", "warning", "error"):
        specs.extend(
            [
                (f"{name}.on_fill", f"on_{name}", f"{name}_fill", 4.5, "text"),
                (
                    f"{name}.text.canvas",
                    f"{name}_text_on_canvas",
                    "canvas_bg",
                    4.5,
                    "text",
                ),
                (
                    f"{name}.text.panel",
                    f"{name}_text_on_panel",
                    "panel_bg",
                    4.5,
                    "text",
                ),
            ]
        )
    return [_pair(theme, "semantic", *spec) for spec in specs]


def ansi_pairs(theme):
    rows = []
    for name in ANSI:
        for prefix, expression in (("normal", name), ("bright", f"bright_{name}")):
            rows.append(
                _pair(
                    theme,
                    "ansi",
                    f"{prefix}.{name}",
                    expression,
                    "canvas_bg",
                    4.5,
                    "raw terminal",
                    False,
                )
            )
    return rows


def kde_pairs(theme):
    spec = load_map("kde")
    rows = []
    for group, (background, alternate) in spec["groups"].items():
        backgrounds = (theme.kde(background), theme.kde(alternate))
        selection = group == "Colors:Selection"
        for key, role in spec["foregrounds"].items():
            if selection:
                color = theme.kde(spec["selection_foregrounds"][key])
            else:
                floor = 3.0 if role == "inactive" else 4.5
                color = theme.readable_many(theme.data["kde"][role], backgrounds, floor)
            floor = 3.0 if role == "inactive" else 4.5
            for label, background_color in (
                ("normal", backgrounds[0]),
                ("alternate", backgrounds[1]),
            ):
                rows.append(
                    Pair(
                        "kde",
                        f"{group}.{key}.{label}",
                        color,
                        background_color,
                        oklab.contrast_ratio(color, background_color),
                        floor,
                        "inactive" if floor == 3 else "text",
                    )
                )
        decoration = theme.readable_many(theme.data["kde"]["decoration"], backgrounds, 3)
        for label, background_color in (("normal", backgrounds[0]), ("alternate", backgrounds[1])):
            rows.append(
                Pair(
                    "kde",
                    f"{group}.decoration.{label}",
                    decoration,
                    background_color,
                    oklab.contrast_ratio(decoration, background_color),
                    3.0,
                    "graphic",
                )
            )
    return rows


def gtk_pairs(theme):
    mapping = load_map("gtk")["colors"]
    specs = [
        ("text.window", "theme_fg_color", "theme_bg_color", 4.5, "text"),
        ("text.view", "theme_text_color", "theme_base_color", 4.5, "text"),
        (
            "text.button",
            "theme_button_foreground_normal",
            "theme_button_background_normal",
            4.5,
            "text",
        ),
        ("text.tooltip", "tooltip_text", "tooltip_background", 4.5, "text"),
        (
            "text.header",
            "theme_header_foreground",
            "theme_header_background",
            4.5,
            "text",
        ),
        (
            "selection",
            "theme_selected_fg_color",
            "theme_selected_bg_color",
            4.5,
            "text",
        ),
        ("negative", "error_color", "theme_base_color", 4.5, "text"),
        ("neutral", "warning_color", "theme_base_color", 4.5, "text"),
        ("positive", "success_color", "theme_base_color", 4.5, "text"),
        ("link", "link_color", "theme_base_color", 4.5, "text"),
        ("visited", "link_visited_color", "theme_base_color", 4.5, "text"),
        (
            "inactive.button",
            "theme_button_foreground_insensitive",
            "theme_button_background_insensitive",
            3.0,
            "inactive",
        ),
        (
            "focus.button",
            "theme_button_decoration_focus",
            "theme_button_background_normal",
            3.0,
            "graphic",
        ),
        (
            "focus.view",
            "theme_view_active_decoration_color",
            "theme_base_color",
            3.0,
            "graphic",
        ),
        ("border.tooltip", "tooltip_border", "tooltip_background", 3.0, "graphic"),
    ]
    rows = []
    for state, foreground_variable, background_variable, floor, kind in specs:
        foreground = mapping[foreground_variable]
        background = mapping[background_variable]
        bg = theme.mapped_color("kde", background)
        fg = theme.mapped_color("kde", foreground)
        rows.append(Pair("gtk", state, fg, bg, oklab.contrast_ratio(fg, bg), floor, kind))
    return rows


def obsidian_pairs(theme):
    specs = [
        ("text.normal", "text_on_canvas", "canvas_bg", 4.5, "text"),
        ("text.muted", "muted_on_canvas", "canvas_bg", 4.5, "text"),
        ("text.faint", "disabled_on_canvas", "canvas_bg", 3.0, "inactive"),
        ("text.accent", "primary_text_on_canvas", "canvas_bg", 4.5, "text"),
        ("text.on_accent", "on_primary", "primary_fill", 4.5, "text"),
        ("titlebar.text", "text_on_panel", "panel_bg", 4.5, "text"),
        ("titlebar.muted", "muted_on_panel", "panel_bg", 4.5, "text"),
        ("border", "border_on_canvas", "canvas_bg", 3.0, "graphic"),
        ("focus", "focus_ring", "canvas_bg", 3.0, "graphic"),
        ("tag.text", "primary_text_on_canvas", "canvas_bg", 4.5, "text"),
    ]
    for name in ("info", "success", "warning", "error"):
        specs.append((f"{name}.text", f"{name}_text_on_canvas", "canvas_bg", 4.5, "text"))
    for name in ANSI:
        specs.append(
            (
                f"code.{name}",
                f"ansi_{name}_text_on_panel",
                "panel_bg",
                4.5,
                "text",
            )
        )
    return [_pair(theme, "obsidian", *spec) for spec in specs]


def yazi_pairs(theme, template):
    return [
        Pair(
            "yazi",
            row.state,
            row.foreground,
            row.background,
            row.ratio,
            row.floor,
            row.kind,
        )
        for row in yazi.contrast_pairs(theme, template)
    ]


def tmux_pairs(theme):
    colors = tmux.colors(theme)
    return [
        Pair(
            "tmux",
            state,
            colors[foreground],
            colors[background],
            oklab.contrast_ratio(colors[foreground], colors[background]),
            floor,
            "graphic" if floor == 3 else "text",
        )
        for state, foreground, background, floor in tmux.color_pairs()
    ]


def required_pairs(theme, yazi_template):
    return [
        *semantic_pairs(theme),
        *kde_pairs(theme),
        *gtk_pairs(theme),
        *obsidian_pairs(theme),
        *yazi_pairs(theme, yazi_template),
        *tmux_pairs(theme),
    ]


def matrix(theme, yazi_template):
    rows = [*required_pairs(theme, yazi_template), *ansi_pairs(theme)]
    lines = [
        f"# Contrast matrix: {theme.name}",
        "",
        f"Generated from `theme/profiles/{theme.profile}.toml`.",
        "Required text pairs target 4.5:1; graphical and inactive pairs target 3:1.",
        "Raw ANSI rows are reported but are not enforced.",
        "",
        "| Area | State | Foreground | Background | Ratio | Floor | Result |",
        "|---|---|---:|---:|---:|---:|---|",
    ]
    for row in rows:
        result = "raw" if not row.enforced else ("pass" if row.passes else "FAIL")
        lines.append(
            f"| {row.area} | `{row.state}` | `{row.foreground}` | `{row.background}` | "
            f"{row.ratio:.2f}:1 | {row.floor:.1f}:1 | {result} |"
        )
    return "\n".join(lines) + "\n"
