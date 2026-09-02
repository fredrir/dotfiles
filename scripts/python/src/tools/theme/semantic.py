from types import MappingProxyType

from tools.theme import derive
from tools.theme.schema import ANSI_KEYS

_SEMANTIC_COLORS = {
    "background": "ui.background",
    "primary": "ui.primary",
    "accent": "ui.accent",
    "sidebar": "ui.accent",
    "surface": "ui.surface",
    "border": "ui.surface",
    "foreground": "ui.foreground",
    "cursor": "ui.foreground",
    "cursor_text": "ui.background",
    "selection_bg": "ui.primary",
    "selection_fg": "contrast(ui.primary)",
}

for _name in ANSI_KEYS:
    _SEMANTIC_COLORS[_name] = f"ansi.normal.{_name}"
    _SEMANTIC_COLORS[f"bright_{_name}"] = f"ansi.bright.{_name}"

_SEMANTIC_COLORS.update(
    {
        "error": "ansi.normal.red",
        "warning": "ansi.normal.yellow",
        "success": "ansi.normal.green",
        "info": "ansi.normal.blue",
        "sunken": "background/-100",
        "separator": "ui.surface~ui.foreground/500",
        "disabled": "background/700",
        "muted": "background/900",
        "orange": "ansi.bright.yellow~ansi.normal.yellow/400",
    }
)

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
