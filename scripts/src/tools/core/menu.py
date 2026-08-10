import os
import select
import sys
import termios
import tty

from tools.core.console import colors_enabled

HIDE = "\033[?25l"
SHOW = "\033[?25h"


def _read_key(fd):
    key = os.read(fd, 1).decode(errors="ignore")
    if key == "\x1b" and select.select([fd], [], [], 0.05)[0]:
        key += os.read(fd, 2).decode(errors="ignore")
    return key


def erase(option_count):
    if not sys.stdout.isatty():
        return
    sys.stdout.write(f"\033[{option_count + 4}A\033[J")
    sys.stdout.flush()


def pick(title, options, descriptions=None, default=0):
    if not options or not sys.stdout.isatty():
        return None
    try:
        with open("/dev/tty", "rb", buffering=0) as handle:
            return _loop(handle, title, options, descriptions, default)
    except OSError:
        return None


def _loop(handle, title, options, descriptions, default):
    bold, dim, cyan, reset = ("\033[1m", "\033[2m", "\033[36m", "\033[0m")
    if not colors_enabled():
        bold = dim = cyan = reset = ""
    fd = handle.fileno()
    count = len(options)
    index = min(max(default, 0), count - 1)
    width = max(len(option) for option in options)
    write = sys.stdout.write
    write(f"\n  {bold}{title}{reset}\n")
    write(f"  {dim}↑/↓ move · enter select · q quit{reset}\n\n")
    write(HIDE)
    saved = termios.tcgetattr(fd)
    try:
        tty.setcbreak(fd)
        first = True
        while True:
            if not first:
                write(f"\033[{count}A")
            first = False
            for position, option in enumerate(options):
                label = option.ljust(width)
                detail = ""
                if descriptions and descriptions[position]:
                    detail = f"  {dim}{descriptions[position]}{reset}"
                if position == index:
                    write(f"  {cyan}{bold}❯ {label}{reset}{detail}\033[K\n")
                else:
                    write(f"    {label}{detail}\033[K\n")
            sys.stdout.flush()
            key = _read_key(fd)
            if key in ("\x1b[A", "k"):
                index = (index + count - 1) % count
            elif key in ("\x1b[B", "j"):
                index = (index + 1) % count
            elif key.isdigit() and 1 <= int(key) <= count:
                index = int(key) - 1
            elif key in ("\r", "\n"):
                return index
            elif key in ("q", "\x1b", ""):
                return None
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, saved)
        write(SHOW)
        sys.stdout.flush()
