"""The rust tools' half of the command tree, read from `--command-dump`.

Same shape as the python side, so `docs.py` cannot tell the two apart. The
binary is asked rather than the source parsed: clap owns defaults, hidden
flags and the `-h/-V` pair it adds itself, and only the built parser knows
all of it.
"""

import os
import subprocess

from tools.core.paths import dotfiles_root
from tools.surface.introspect import Command, Param, one_line

# The name a page calls the tool, against the file that answers to it.
BINARIES = {"gdd": "git-discard"}


def binary(program):
    """The built tool, preferring this checkout's over whatever is installed."""
    name = BINARIES.get(program, program)
    candidates = (
        os.path.join(str(dotfiles_root()), "scripts/rust/target/release", name),
        os.path.join(os.path.expanduser("~/.local/bin"), name),
    )
    for path in candidates:
        if os.access(path, os.X_OK):
            return path
    return ""


def tree(program):
    """The tree of one rust tool, or None when it has not been built here."""
    path = binary(program)
    if not path:
        return None
    result = subprocess.run(
        [path, "--command-dump"], capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        return None
    return from_dump(result.stdout, program)


def from_dump(text, program):
    commands = {}
    order = []
    for line in text.splitlines():
        fields = line.split("\t")
        if fields[0] == "C" and len(fields) >= 4:
            path = tuple(fields[1].split(" "))
            commands[path] = {"help": one_line(fields[3]), "hidden": fields[2] == "1", "params": []}
            order.append(path)
        elif fields[0] == "A" and len(fields) >= 9:
            path = tuple(fields[1].split(" "))
            if path in commands:
                commands[path]["params"].append(_param(fields))
    if not order:
        return None
    return _build(order[0], commands, program)


def _param(fields):
    _kind, _path, kind, name, spellings, metavar, multiple, required, hidden, help_text = fields[:10]
    opts = tuple(spelling for spelling in spellings.split(",") if spelling)
    return Param(
        kind=kind,
        name=name,
        opts=opts,
        secondary=(),
        metavar=metavar,
        help=one_line(help_text),
        multiple=multiple == "1",
        required=required == "1",
        hidden=hidden == "1",
    )


def _build(path, commands, program):
    entry = commands[path]
    children = tuple(
        _build(other, commands, program)
        for other in commands
        if len(other) == len(path) + 1 and other[: len(path)] == path
    )
    return Command(
        path=(program,) + path[1:],
        help=entry["help"],
        hidden=entry["hidden"],
        params=tuple(entry["params"]),
        children=children,
    )
