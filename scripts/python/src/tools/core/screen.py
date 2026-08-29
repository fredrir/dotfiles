import contextlib
import os
import re
import select
import termios
import tty

BOLD = "\033[1m"
DIM = "\033[2m"
CYAN = "\033[36m"
GREEN = "\033[32m"
RED = "\033[31m"
YELLOW = "\033[33m"
RESET = "\033[0m"

HIDE = "\033[?25l"
SHOW = "\033[?25h"
CLEAR = "\033[K"
ERASE = "\033[J"
CURSOR = "❯ "
BLANK = "  "
INDENT = "  "
GAP = "  "
ESC_DELAY = 0.05
ESC_LIMIT = 16
ESC_DRAIN = 256
CSI_FINAL = "\x40", "\x7e"

ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")

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


def paint(text, color, color_on):
    return f"{color}{text}{RESET}" if color_on else text


def visible(text):
    return len(ANSI.sub("", text))


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


def normalise(key):
    return ARROWS.get(key) or NAMED.get(key, key)


def pending(fd):
    return bool(select.select([fd], [], [], ESC_DELAY)[0])


def read_byte(fd):
    """One byte as text.  An undecodable byte still reads as a character, so only a real
    end of stream comes back empty and the caller can tell the two apart."""
    data = os.read(fd, 1)
    return data.decode(errors="replace") if data else ""


def read_escape(fd):
    """The rest of an escape sequence: SS3 takes one final byte, CSI runs to its final
    byte.  An overlong CSI is drained to the end even though only the first ESC_LIMIT
    characters are reported, so no tail is left to read back as a keystroke of its own."""
    key = "\033" + read_byte(fd)
    if key == "\033O":
        return key + (read_byte(fd) if pending(fd) else "")
    if key != "\033[":
        return key
    low, high = CSI_FINAL
    for _step in range(ESC_DRAIN):
        if not pending(fd):
            break
        byte = read_byte(fd)
        if not byte:
            break
        if len(key) < ESC_LIMIT:
            key += byte
        if low <= byte <= high:
            break
    return key


def read_key(fd):
    """One keystroke.  A bare ESC returns at once rather than waiting for a sequence, and
    a terminal that hangs up reads as "" so the caller treats it as an abort."""
    try:
        key = read_byte(fd)
        if key == "\033" and pending(fd):
            return read_escape(fd)
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
