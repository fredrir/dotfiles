from tools.dotfile.state import log

BOLD = "\033[1m"
DIM = "\033[2m"
GREEN = "\033[32m"
RED = "\033[31m"
YELLOW = "\033[33m"
RESET = "\033[0m"

MARKS = {"ok": ("✓", GREEN), "bad": ("✗", RED), "warn": ("!", YELLOW), "note": ("·", DIM)}

INDENT = "  "
ITEM_INDENT = " " * 6
LABEL_WIDTH = 11
ITEM_LIMIT = 12


def paint(text, color, color_on):
    return f"{color}{text}{RESET}" if color_on else text


def plural(count, noun, many=""):
    return f"{count} {noun}" if count == 1 else f"{count} {many or noun + 's'}"


def clip(items, show_all):
    if show_all or len(items) <= ITEM_LIMIT:
        return items
    return items[:ITEM_LIMIT] + [(f"+{len(items) - ITEM_LIMIT} more", "")]


def align(items):
    width = max((len(text) for text, note in items if note), default=0)
    return [(f"{text:<{width}}" if note else text, note) for text, note in items]


def row(kind, label, summary, items=(), show_all=False):
    return (kind, label, summary, clip(align(list(items)), show_all))


def emit(entry, color_on):
    kind, label, summary, items = entry
    mark, color = MARKS[kind]
    log(INDENT + paint(mark, color, color_on) + f" {label:<{LABEL_WIDTH}}" + summary)
    for text, note in items:
        log(ITEM_INDENT + text + ("  " + paint(note, DIM, color_on) if note else ""))


def heading(label, detail, color_on):
    log(INDENT + paint(label, DIM, color_on) + "  " + paint(detail, BOLD, color_on))
