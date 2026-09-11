import collections
import contextlib
import shutil
import sys

from tools.transcript.screen import (
    ABORT,
    BLANK,
    BOLD,
    CLEAR,
    CURSOR,
    CYAN,
    DIM,
    ERASE,
    GAP,
    HIDE,
    INDENT,
    SHOW,
    colors_for,
    compose,
    flush,
    normalise,
    scripted,
    tty_keys,
)

GUTTER = "    "
DIGITS = "123456789"

HEADER = 4
SPARE = 1
MIN_ROWS = 3
MIN_WIDTH = 24

HINT = "↑/↓ move | ↩ select | q quit"
DEEP_HINT = "↑/↓ move | ←/→ level | ↩ select | q quit"

Pick = collections.namedtuple("Pick", "kind index option")

_stream = None
_height = 0


class Column:
    def __init__(self, options, details=None, default=0, kind=""):
        self.options = list(options)
        details = list(details or [])[: len(self.options)]
        self.details = details + [""] * (len(self.options) - len(details))
        self.kind = kind
        self.index = min(max(default, 0), max(len(self.options) - 1, 0))

    def label_width(self):
        return max(len(option) for option in self.options)

    def width(self, detailed):
        room = len(CURSOR) + self.label_width()
        if detailed and any(self.details):
            room += len(GAP) + max(len(detail) for detail in self.details)
        return room


class Cascade:
    def __init__(self, title, expand, out, color_on):
        self.title = title
        self.expand = expand
        self.out = out
        self.color_on = color_on
        self.columns = []
        self.opened = {}
        self.height = 0

    def picks(self, depth=None):
        columns = self.columns if depth is None else self.columns[:depth]
        return tuple(Pick(one.kind, one.index, one.options[one.index]) for one in columns)

    def open(self, path):
        if path not in self.opened:
            self.opened[path] = self.expand(path)
        return self.opened[path]

    def descend(self):
        child = self.open(self.picks())
        if child is None or not child.options:
            return False
        self.columns.append(child)
        return True

    def restore(self, start):
        for depth, index in enumerate(start):
            column = self.columns[depth]
            column.index = min(max(index, 0), len(column.options) - 1)
            if depth + 1 >= len(start) or not self.descend():
                return

    def stack(self, limit):
        offsets = []
        for position, column in enumerate(self.columns):
            if not position:
                offsets.append(0)
                continue
            previous = self.columns[position - 1]
            offset = min(offsets[-1] + previous.index, limit - len(column.options))
            offsets.append(max(0, offset))
        return offsets, max(o + len(c.options) for o, c in zip(offsets, self.columns))

    def cell(self, column, index, label, active):
        if index < 0 or index >= len(column.options):
            return [], 0
        chosen = index == column.index
        if active:
            mark = tint = CYAN + BOLD if chosen else ""
        else:
            mark, tint = (CYAN if chosen else ""), ("" if chosen else DIM)
        option = column.options[index]
        segments = [(CURSOR if chosen else BLANK, mark), (option, tint)]
        used = len(CURSOR) + len(option)
        detail = column.details[index] if active else ""
        if detail:
            segments.append((" " * (label - len(option)) + GAP, ""))
            segments.append((detail, DIM))
            used += label - len(option) + len(GAP) + len(detail)
        return segments, used

    def row(self, index, offsets, widths, first, avail):
        last = len(self.columns) - 1
        segments = [(INDENT, "")]
        for position in range(first, last + 1):
            if position > first:
                segments.append((GUTTER, ""))
            column = self.columns[position]
            active = position == last
            cell, used = self.cell(column, index - offsets[position], column.label_width(), active)
            segments.extend(cell)
            if not active:
                segments.append((" " * (widths[position] - used), ""))
        return compose(segments, avail, self.color_on)[0].rstrip()

    def trimmed(self, widths, avail):
        first = 0
        while first < len(self.columns) - 1:
            gutters = len(GUTTER) * (len(self.columns) - 1 - first)
            if len(INDENT) + sum(widths[first:]) + gutters <= avail:
                break
            first += 1
        return first

    def frame(self):
        avail = max(shutil.get_terminal_size().columns - 1, MIN_WIDTH)
        budget = max(shutil.get_terminal_size().lines - HEADER - SPARE, MIN_ROWS)
        offsets, height = self.stack(budget)
        last = len(self.columns) - 1
        widths = [column.width(position == last) for position, column in enumerate(self.columns)]
        first = self.trimmed(widths, avail)
        if first:
            lift = min(offsets[first:])
            offsets = [offset - lift for offset in offsets]
            height = max(offsets[at] + len(self.columns[at].options) for at in range(first, last + 1))
        heading = [(INDENT, ""), (self.title, BOLD)]
        if first:
            walked = " › ".join(one.options[one.index] for one in self.columns[:first])
            heading.append((f"  ‹ {walked}", DIM))
        hint = DEEP_HINT if len(self.columns) > 1 else HINT
        lines = [
            "",
            compose(heading, avail, self.color_on)[0],
            compose([(INDENT, ""), (hint, DIM)], avail, self.color_on)[0],
            "",
        ]
        lines += [self.row(index, offsets, widths, first, avail) for index in range(height)]
        return lines

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

    def wipe(self):
        if not self.height:
            return
        self.out.write(f"\033[{self.height}A" + ERASE)
        self.height = 0
        flush(self.out)

    def summary(self, picks):
        avail = max(shutil.get_terminal_size().columns - 1, MIN_WIDTH)
        steps = [pick.option for pick in picks]
        segments = [(INDENT, ""), (self.title, DIM), (" — ", DIM)]
        if len(steps) > 1:
            segments.append((" › ".join(steps[:-1]) + " › ", DIM))
        segments.append((steps[-1], CYAN))
        return compose(segments, avail, self.color_on)[0]

    def run(self, reader, start):
        root = self.open(())
        if root is None or not root.options:
            return None
        self.columns.append(root)
        self.restore(tuple(start))
        while True:
            self.draw()
            key = normalise(reader())
            if key in ABORT:
                return None
            column = self.columns[-1]
            count = len(column.options)
            if key == "up":
                column.index = (column.index - 1) % count
            elif key == "down":
                column.index = (column.index + 1) % count
            elif key == "left":
                if len(self.columns) > 1:
                    self.columns.pop()
            elif key in ("right", "enter"):
                if not self.descend() and key == "enter":
                    return list(self.picks())
            elif key and key in DIGITS and int(key) <= count:
                column.index = int(key) - 1


def cascade(title, expand, start=(), keys=None, out=None):
    global _stream, _height
    stream = sys.stdout if out is None else out
    if out is None and not stream.isatty():
        return None
    runner = Cascade(title, expand, stream, colors_for(stream))
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
            picks = runner.run(reader, start)
        except KeyboardInterrupt:
            picks = None
        finally:
            runner.wipe()
        _stream, _height = stream, 0
        if picks:
            stream.write(runner.summary(picks) + CLEAR + "\n")
            _height = 1
        return picks


def erase():
    global _height
    if not _height or _stream is None:
        return
    _stream.write(f"\033[{_height}A" + ERASE)
    flush(_stream)
    _height = 0
