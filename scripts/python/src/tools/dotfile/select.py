"""Interactive resolver for the local edits `dotfile sync` finds in a materialised config.

Two axes: ↑/↓ (k/j) walk the changed keys, ←/→ (h/l) walk the action bar, and ⏎ applies
the highlighted action to the highlighted key then advances to the next undecided one.
`target` drills into a submenu of overlay names aligned under it; `‹ back` leaves it.
Digits jump straight to an action, as they do in setup.sh's pick().

Every decision is staged: nothing is returned until all keys are decided *and* the plan
is confirmed, so `q`, ESC or Ctrl-C at any point is a clean abort that yields no result.

Keys are read from /dev/tty and the frame is painted to `out`; both are injectable, so
the whole interaction can be driven by a scripted key sequence in tests.
"""

import contextlib
import os
import select
import shutil
import sys
import termios
import tty

from tools.dotfile.report import BOLD, DIM, GREEN, MARKS, RED, YELLOW, paint, plural

CYAN = "\033[36m"

ACTIONS = ("shared", "target", "ignore", "discard")

GLYPHS = {
    "add": ("+", GREEN),
    "modify": ("~", YELLOW),
    "delete": ("-", RED),
    "conflict": ("!", RED + BOLD),
}
UNKNOWN = ("·", DIM)

TINT = {"shared": GREEN, "target": CYAN, "ignore": YELLOW, "discard": DIM}

BACK = "‹ back"
REVISE = "‹ revise"
HIDE = "\033[?25l"
SHOW = "\033[?25h"
CLEAR = "\033[K"
ERASE = "\033[J"
CURSOR = "❯ "
BLANK = "  "
INDENT = "  "
GAP = "  "
ARROW = "  → "
BAR = 4
ESC_DELAY = 0.05

# blank, title, help, blank, destination, two scroll hints, two footer lines, one spare
CHROME = 10
MIN_ROWS = 3
MIN_WIDTH = 24
LABEL_SHARE = 3, 5  # at most three fifths of a row goes to the key

EDIT, CONFIRM, DONE, ABORTED = "edit", "confirm", "done", "aborted"
SUB = "sub"

HELP = {
    EDIT: "↑/↓ key · ←/→ action · ⏎ apply · a rest · u undo · q abort",
    SUB: "←/→ target · ⏎ choose · a rest · ‹ back · q abort",
    CONFIRM: "↑/↓ review · ←/→ choose · ⏎ confirm · u undo · q abort",
    DONE: "",
    ABORTED: "",
}

ARROWS = {
    "\033[A": "up",
    "\033OA": "up",
    "\033[B": "down",
    "\033OB": "down",
    "\033[C": "right",
    "\033OC": "right",
    "\033[D": "left",
    "\033OD": "left",
}
NAMED = {"k": "up", "j": "down", "h": "left", "l": "right", "\r": "enter", "\n": "enter"}
ABORT = ("q", "\033", "\003", "")


class Change:
    """One local edit to route.  `kind` picks the row glyph, `label` and `detail` are
    pre-rendered for display, and `targets` are the overlay names the submenu offers."""

    def __init__(self, kind, path, label, detail="", targets=()):
        self.kind = kind
        self.path = tuple(path)
        self.label = label
        self.detail = detail
        self.targets = list(targets)

    def __repr__(self):
        return f"Change({self.kind!r}, {self.path!r}, {self.label!r})"


def fit(text, width):
    """Clip to `width` columns, marking the cut with an ellipsis."""
    if width <= 0:
        return ""
    if len(text) <= width:
        return text
    return text[: width - 1] + "…" if width > 1 else "…"


def compose(segments, width, color_on):
    """Paint (text, colour) segments into one line; returns the line and its printed width."""
    parts, used = [], 0
    for text, color in segments:
        if used >= width:
            break
        text = fit(text, width - used)
        used += len(text)
        parts.append(paint(text, color, color_on) if color else text)
    return "".join(parts), used


def cells(items, index, column, focused):
    """Bar segments: equal-width cells, a `❯ ` cursor on the highlighted one.

    Returns the segments and the stride between cell columns, so a submenu can be
    aligned under the primary action it belongs to."""
    span = max(len(item) for item in items) + len(CURSOR)
    segments = [(" " * column, "")]
    for position, item in enumerate(items):
        text = (CURSOR if position == index else BLANK) + item
        if position + 1 < len(items):
            text = text.ljust(span)
        if position == index:
            color = CYAN + BOLD if focused else CYAN
        else:
            color = "" if focused else DIM
        segments.append((text, color))
        if position + 1 < len(items):
            segments.append((" ", ""))
    return segments, span + 1


def normalise(key):
    return ARROWS.get(key) or NAMED.get(key, key)


def read_key(fd):
    """One keystroke.  A bare ESC returns at once rather than waiting for a sequence, and
    a terminal that hangs up reads as "" so the caller treats it as an abort."""
    try:
        data = os.read(fd, 1)
        if not data:
            return ""
        key = data.decode(errors="ignore")
        if key == "\033" and select.select([fd], [], [], ESC_DELAY)[0]:
            key += os.read(fd, 2).decode(errors="ignore")
    except OSError:
        return ""
    return key


@contextlib.contextmanager
def tty_keys():
    """A cbreak reader on /dev/tty, so the selector still works with stdout redirected."""
    with open("/dev/tty", "rb", buffering=0) as handle:
        fd = handle.fileno()
        saved = termios.tcgetattr(fd)
        try:
            tty.setcbreak(fd)
            yield lambda: read_key(fd)
        finally:
            termios.tcsetattr(fd, termios.TCSADRAIN, saved)


def scripted(keys):
    """Accept either a reader callable or a plain sequence of keystrokes."""
    if callable(keys):
        return keys
    pending = iter(keys)
    return lambda: next(pending, "")


def colors_for(stream):
    if "NO_COLOR" in os.environ:
        return False
    isatty = getattr(stream, "isatty", None)
    return bool(isatty and isatty())


def flush(stream):
    handler = getattr(stream, "flush", None)
    if handler:
        handler()


class Selector:
    def __init__(self, dst, changes, out, color_on):
        self.dst = dst
        self.changes = changes
        self.out = out
        self.color_on = color_on
        self.decisions = {}
        self.history = []
        self.row = 0
        self.action = 0
        self.sub = None
        self.bulk = False
        self.choice = 0
        self.top = 0
        self.height = 0
        self.mode = EDIT
        self.widest_label = max(len(change.label) for change in changes)
        self.widest_detail = max(len(change.detail) for change in changes)
        names = ["shared", "ignore", "discard"]
        names += [f"target:{name}" for change in changes for name in change.targets]
        self.plan = max(len(name) for name in names) + len(ARROW)

    # ---- decisions -------------------------------------------------------

    def undecided(self):
        return [index for index in range(len(self.changes)) if index not in self.decisions]

    def options(self):
        return self.changes[self.row].targets

    def items(self):
        return [BACK] + self.options()

    def rows_for(self, bulk):
        return self.undecided() if bulk else [self.row]

    def commit(self, rows, decision):
        """Stage `decision` on every row it applies to, as one undoable batch."""
        wanted = decision.partition(":")[2]
        batch = []
        for index in rows:
            if wanted and wanted not in self.changes[index].targets:
                continue
            batch.append((index, self.decisions.get(index)))
            self.decisions[index] = decision
        if not batch:
            return
        self.history.append(batch)
        self.advance(max(index for index, _previous in batch))
        if len(self.decisions) == len(self.changes):
            self.mode = CONFIRM
            self.choice = 0

    def advance(self, start):
        count = len(self.changes)
        for step in range(1, count + 1):
            index = (start + step) % count
            if index not in self.decisions:
                self.row = index
                return

    def undo(self):
        if not self.history:
            return
        batch = self.history.pop()
        for index, previous in reversed(batch):
            if previous is None:
                self.decisions.pop(index, None)
            else:
                self.decisions[index] = previous
        self.row = batch[0][0]
        self.sub = None
        if len(self.decisions) < len(self.changes):
            self.mode = EDIT

    # ---- keys ------------------------------------------------------------

    def edit(self, key):
        count = len(self.changes)
        if key == "up":
            self.row = (self.row + count - 1) % count
        elif key == "down":
            self.row = (self.row + 1) % count
        elif key == "left":
            self.action = (self.action + len(ACTIONS) - 1) % len(ACTIONS)
        elif key == "right":
            self.action = (self.action + 1) % len(ACTIONS)
        elif key.isdigit() and 1 <= int(key) <= len(ACTIONS):
            self.action = int(key) - 1
        elif key == "enter":
            self.choose(False)
        elif key == "a":
            self.choose(True)
        elif key == "u":
            self.undo()

    def choose(self, bulk):
        name = ACTIONS[self.action]
        if name != "target":
            self.commit(self.rows_for(bulk), name)
        elif self.options():
            self.bulk = bulk
            self.sub = 1

    def drill(self, key):
        items = self.items()
        size = len(items)
        if key == "left":
            self.sub = (self.sub + size - 1) % size
        elif key == "right":
            self.sub = (self.sub + 1) % size
        elif key.isdigit() and 1 <= int(key) <= size:
            self.sub = int(key) - 1
        elif key == "enter":
            self.pick(self.bulk)
        elif key == "a":
            self.pick(True)
        elif key in ("up", "down"):
            self.sub = None
            self.edit(key)
        elif key == "u":
            self.sub = None
            self.undo()

    def pick(self, bulk):
        items = self.items()
        chosen = self.sub
        self.sub = None
        if chosen:
            self.commit(self.rows_for(bulk), f"target:{items[chosen]}")

    def confirmed(self, key):
        count = len(self.changes)
        if key == "up":
            self.row = (self.row + count - 1) % count
        elif key == "down":
            self.row = (self.row + 1) % count
        elif key in ("left", "right"):
            self.choice = 1 - self.choice
        elif key.isdigit() and 1 <= int(key) <= 2:
            self.choice = int(key) - 1
        elif key == "u":
            self.undo()
        elif key == "enter":
            if not self.choice:
                return True
            self.mode = EDIT
        return False

    def run(self, keys):
        self.draw()
        while True:
            key = normalise(keys())
            if key in ABORT:
                self.abort()
                return None
            if self.mode == CONFIRM:
                if self.confirmed(key):
                    self.mode = DONE
                    self.draw()
                    return dict(sorted(self.decisions.items()))
            elif self.sub is None:
                self.edit(key)
            else:
                self.drill(key)
            self.settle()
            self.draw()

    def abort(self):
        self.mode = ABORTED
        self.sub = None
        self.draw()

    # ---- painting --------------------------------------------------------

    def window(self):
        room = max(shutil.get_terminal_size().lines - CHROME, MIN_ROWS)
        return min(len(self.changes), room)

    def settle(self):
        window = self.window()
        if self.row < self.top:
            self.top = self.row
        elif self.row >= self.top + window:
            self.top = self.row - window + 1
        self.top = max(0, min(self.top, len(self.changes) - window))

    def text(self, segments, width):
        return compose(segments, width, self.color_on)[0]

    def title(self, width):
        parts = ["dotfile sync", plural(len(self.changes), "change")]
        if self.decisions:
            parts.append(f"{len(self.decisions)} decided")
        return self.text([(INDENT, ""), (" · ".join(parts), BOLD)], width)

    def hint(self, count, where, width):
        if count <= 0:
            return ""
        return self.text([(" " * BAR, ""), (f"⋯ {count} more {where}", DIM)], width)

    def row_line(self, index, width):
        change = self.changes[index]
        glyph, tint = GLYPHS.get(change.kind, UNKNOWN)
        current = index == self.row and self.mode in (EDIT, CONFIRM)
        room = max(width - self.plan, MIN_WIDTH)
        share, whole = LABEL_SHARE
        label = max(8, room * share // whole)
        label = min(self.widest_label, label)
        body = min(len(INDENT) + len(CURSOR) + 2 + label + len(GAP) + self.widest_detail, room)
        segments = [
            (INDENT + (CURSOR if current else BLANK), CYAN + BOLD if current else ""),
            (glyph + " ", tint),
            (fit(change.label, label).ljust(label) + GAP, BOLD if current else ""),
            (change.detail, DIM),
        ]
        line, used = compose(segments, body, self.color_on)
        decision = self.decisions.get(index)
        if not decision:
            return line.rstrip()
        plan = [(ARROW, DIM), (decision, TINT[decision.partition(":")[0]])]
        return line + " " * (body - used) + self.text(plan, width - body)

    def footer(self, width):
        if self.mode in (DONE, ABORTED):
            return [self.closing(width), ""]
        if self.mode == CONFIRM:
            items = [f"apply {plural(len(self.changes), 'change')}", REVISE]
            segments, _stride = cells(items, self.choice, BAR, True)
            return [self.text(segments, width), ""]
        segments, stride = cells(list(ACTIONS), self.action, BAR, self.sub is None)
        bar = self.text(segments, width)
        if self.sub is None:
            return [bar, ""]
        under, _stride = cells(self.items(), self.sub, BAR + self.action * stride, True)
        return [bar, self.text(under, width)]

    def closing(self, width):
        if self.mode == DONE:
            mark, tint = MARKS["ok"]
            note = f"applying {plural(len(self.changes), 'change')}"
        else:
            mark, tint = MARKS["bad"]
            note = "aborted · nothing applied"
        return self.text([(" " * BAR, ""), (mark + " ", tint), (note, DIM)], width)

    def frame(self):
        width = max(shutil.get_terminal_size().columns - 1, MIN_WIDTH)
        window = self.window()
        mode = SUB if self.sub is not None else self.mode
        guide = HELP[mode]
        lines = [
            "",
            self.title(width),
            self.text([(INDENT, ""), (guide, DIM)], width) if guide else "",
            "",
            self.text([(INDENT, ""), (self.dst, DIM)], width),
            self.hint(self.top, "above", width),
        ]
        lines += [self.row_line(index, width) for index in range(self.top, self.top + window)]
        lines.append(self.hint(len(self.changes) - self.top - window, "below", width))
        return lines + self.footer(width)

    def draw(self):
        lines = self.frame()
        write = self.out.write
        if self.height:
            write(f"\033[{self.height}A")
        for line in lines:
            write(line + CLEAR + "\n")
        if len(lines) < self.height:
            write(ERASE)
        self.height = len(lines)
        flush(self.out)


def resolve(dst_label, changes, keys=None, out=None):
    """Route each change interactively.

    Returns {index: decision} once every change is decided and the plan is confirmed,
    where decision is "shared", "target:<name>", "ignore" or "discard".  Returns None
    if the user aborted or no terminal is available, having staged nothing."""
    changes = list(changes)
    if not changes:
        return {}
    stream = sys.stdout if out is None else out
    selector = Selector(dst_label, changes, stream, colors_for(stream))
    with contextlib.ExitStack() as stack:
        if keys is None:
            try:
                reader = stack.enter_context(tty_keys())
            except OSError:
                return None
        else:
            reader = scripted(keys)
        stack.callback(flush, stream)
        stack.callback(stream.write, SHOW)
        stream.write(HIDE)
        try:
            return selector.run(reader)
        except KeyboardInterrupt:
            selector.abort()
            return None
