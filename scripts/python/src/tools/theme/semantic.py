from types import MappingProxyType

from tools.theme import derive
from tools.theme.schema import ANSI_KEYS

_SEMANTIC_COLORS = {
    # Primitive compatibility names. Their meaning is intentionally literal.
    "background": "ui.background",
    "primary": "ui.primary",
    "accent": "ui.accent",
    "surface": "ui.surface",
    "foreground": "ui.foreground",
    "sidebar": "ui.accent",
    "border": "ui.surface",
    "error": "ansi.normal.red",
    "warning": "ansi.normal.yellow",
    "success": "ansi.normal.green",
    "info": "ansi.normal.blue",
    # Intent roles.
    "canvas_bg": "ui.background",
    "panel_bg": "ui.accent",
    "surface_fill": "ui.surface",
    "primary_fill": "ui.primary",
    "primary_hover_fill": "ui.primary~ui.foreground/150",
    "text_on_canvas": "readable(ui.foreground,ui.background,4.5)",
    "text_on_panel": "readable(ui.foreground,ui.accent,4.5)",
    "text_on_surface": "readable(ui.foreground,ui.surface,4.5)",
    "on_primary": "on(ui.primary,4.5)",
    "on_primary_hover": "on(primary_hover_fill,4.5)",
    "primary_text_on_canvas": "readable(ui.primary,ui.background,4.5)",
    "primary_text_on_panel": "readable(ui.primary,ui.accent,4.5)",
    "primary_fill_on_canvas": "visible(ui.primary,ui.background,3)",
    "primary_fill_on_panel": "visible(ui.primary,ui.accent,3)",
    "primary_visible_on_surface": "visible(ui.primary,ui.surface,3)",
    "on_primary_fill_on_canvas": "on(primary_fill_on_canvas,4.5)",
    "on_primary_fill_on_panel": "on(primary_fill_on_panel,4.5)",
    "focus_ring": "visible(ui.primary,ui.background,3)",
    "focus_ring_on_surface": "visible(ui.primary,ui.surface,3)",
    "border_on_canvas": "visible(ui.surface,ui.background,3)",
    "border_on_panel": "visible(ui.surface,ui.accent,3)",
    "border_on_surface": "visible(ui.accent,ui.surface,3)",
    "sunken": "ui.background/-100",
    "muted_on_canvas": "readable(ui.background~ui.foreground/600,ui.background,4.5)",
    "muted_on_panel": "readable(ui.accent~ui.foreground/600,ui.accent,4.5)",
    "disabled_on_canvas": "visible(ui.background~ui.foreground/450,ui.background,3)",
    "disabled_on_panel": "visible(ui.accent~ui.foreground/450,ui.accent,3)",
    "disabled_on_surface": "visible(ui.surface~ui.foreground/450,ui.surface,3)",
    "separator": "border_on_canvas",
    "muted": "muted_on_canvas",
    "disabled": "disabled_on_canvas",
    "cursor": "ui.foreground",
    "cursor_text": "ui.background",
    "selection_bg": "primary_fill",
    "selection_fg": "on_primary",
    "orange": "ansi.bright.yellow~ansi.normal.yellow/400",
}

for _name in ANSI_KEYS:
    _SEMANTIC_COLORS[_name] = f"ansi.normal.{_name}"
    _SEMANTIC_COLORS[f"bright_{_name}"] = f"ansi.bright.{_name}"
    for _group, _source in (
        ("ansi", f"ansi.normal.{_name}"),
        ("bright_ansi", f"ansi.bright.{_name}"),
    ):
        _SEMANTIC_COLORS[f"{_group}_{_name}_text_on_canvas"] = (
            f"readable({_source},ui.background,4.5)"
        )
        _SEMANTIC_COLORS[f"{_group}_{_name}_text_on_panel"] = f"readable({_source},ui.accent,4.5)"

for _name, _source in {
    "error": "ansi.normal.red",
    "warning": "ansi.normal.yellow",
    "success": "ansi.normal.green",
    "info": "ansi.normal.blue",
}.items():
    _SEMANTIC_COLORS[f"{_name}_fill"] = _source
    _SEMANTIC_COLORS[f"on_{_name}"] = f"on({_source},4.5)"
    _SEMANTIC_COLORS[f"{_name}_text_on_canvas"] = f"readable({_source},ui.background,4.5)"
    _SEMANTIC_COLORS[f"{_name}_text_on_panel"] = f"readable({_source},ui.accent,4.5)"
    _SEMANTIC_COLORS[f"{_name}_visible_on_panel"] = f"visible({_source},ui.accent,3)"
    _SEMANTIC_COLORS[f"{_name}_fill_on_canvas"] = f"visible({_source},ui.background,3)"
    _SEMANTIC_COLORS[f"on_{_name}_fill_on_canvas"] = f"on({_name}_fill_on_canvas,4.5)"

SEMANTIC_COLORS = MappingProxyType(_SEMANTIC_COLORS)


def resolve_semantics(primitives, semantics=SEMANTIC_COLORS):
    resolved = {}
    stack = []
    background = primitives["ui.background"]
    foreground = primitives["ui.foreground"]

    def lookup(name):
        if name in primitives:
            return primitives[name]
        if name in resolved:
            return resolved[name]
        if name not in semantics:
            raise SystemExit(f"unknown palette color: {name}")
        if name in stack:
            cycle = " -> ".join([*stack[stack.index(name) :], name])
            raise SystemExit(f"semantic color cycle: {cycle}")
        stack.append(name)
        try:
            resolved[name] = derive.resolve(semantics[name], lookup, background, foreground).hex
        finally:
            stack.pop()
        return resolved[name]

    for name in semantics:
        lookup(name)
    return resolved
