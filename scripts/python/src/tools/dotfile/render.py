"""Shared one-line rendering of JSON config values.

`dotfile status` and `dotfile sync` describe the same thing — a key of a materialised
config that no longer matches the repo — so both format it here: `key` for the key
column, `change` for the detail column, and `value` for a lone value.

Everything collapses onto one line and clips to a width budget with an ellipsis, and a
clip never cuts a backslash escape in half.
"""

import json

ELLIPSIS = "…"
SEPARATOR = "/"
JOIN = " → "
REMOVED = "(removed)"
OURS = "repo: "
THEIRS = "  live: "
COMPACT = (", ", ": ")
UNICODE = 6
ESCAPE = 2


def dumps(item):
    """Compact one-line JSON: ["a", "b"], {"x": 1}, "quoted", true, null."""
    return json.dumps(item, ensure_ascii=False, separators=COMPACT, default=str)


def step(text, index):
    """How far the atom at `index` reaches: a backslash escape counts as one atom."""
    if text[index] != "\\":
        return 1
    return UNICODE if text[index + 1 : index + 2] == "u" else ESCAPE


def fit(text, width):
    """Clip to `width`, marking the cut with an ellipsis.  A width of None never clips,
    and the cut always lands on an atom boundary so no escape is left half-written."""
    if width is None or len(text) <= width:
        return text
    if width <= 1:
        return ELLIPSIS if width == 1 else ""
    cut = index = 0
    while index <= width - 1:
        cut = index
        index += step(text, index)
    return text[:cut] + ELLIPSIS


def split(budget, first, second):
    """Share `budget` between two sides.  Whichever side fits whole keeps its full
    length and hands the surplus over, so a short side never starves a long one."""
    if len(first) + len(second) <= budget:
        return len(first), len(second)
    half = budget // 2
    if len(first) <= half:
        return len(first), budget - len(first)
    if len(second) <= budget - half:
        return budget - len(second), len(second)
    return half, budget - half


def room(width, fixed):
    """What is left of `width` once the fixed part of a line is spoken for."""
    return None if width is None else max(width - len(fixed), 0)


def sides(ours, theirs, lead, join, width):
    """Two values either side of `join`, each clipped to a fair share of the width."""
    first, second = dumps(ours), dumps(theirs)
    if width is None:
        return lead + first + join + second
    left, right = split(room(width, lead + join), first, second)
    return fit(lead + fit(first, left) + join + fit(second, right), width)


def value(item, width=None):
    """One line for a single JSON value."""
    return fit(dumps(item), width)


def key(path):
    """The display form of a key path.  `/` nests, matching the ignore patterns the user
    types in merge.dotfile; dots are left alone because "editor.formatOnSave" is one key."""
    return dumps(SEPARATOR.join(str(segment) for segment in path))


def change(kind, ours, theirs, width=None):
    """The detail column for one changed key.

    add       the new value          modify    old → new
    delete    old → (removed)        conflict  repo: ours  live: theirs

    Anything else reads as a modify.  "(removed)" and the labels are fixed furniture and
    are never clipped; the rest of the width is shared between the two values."""
    if kind == "add":
        return value(theirs, width)
    if kind == "delete":
        tail = JOIN + REMOVED
        return fit(fit(dumps(ours), room(width, tail)) + tail, width)
    if kind == "conflict":
        return sides(ours, theirs, OURS, THEIRS, width)
    return sides(ours, theirs, "", JOIN, width)
