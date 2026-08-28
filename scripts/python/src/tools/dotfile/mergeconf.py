"""merge.dotfile: per-package directives naming keys a merge must leave alone.

    # keep whatever this machine decided
    ignore  workbench.colorTheme
    ignore  cSpell.*
    ignore  [lua]/editor.tabSize

`/` separates nesting levels. A dot is an ordinary character inside a key name,
because VS Code writes flat keys like "editor.formatOnSave" beside nested ones
like "[lua]", so it cannot double as the separator. A pattern that names a key
covers everything below it as well.
"""

import fnmatch
import os

from tools.dotfile.state import die, trim

NAME = "merge.dotfile"
IGNORE = "ignore"


def read_directives(path):
    found = []
    with open(path, encoding="utf-8") as handle:
        for line in handle.read().splitlines():
            directive = trim(line.split("#", 1)[0])
            if not directive:
                continue
            fields = directive.split(None, 1)
            if fields[0] != IGNORE or len(fields) != 2:
                die(f"{path}: expected 'ignore <pattern>', got '{directive}'")
            found.append(fields[1])
    return found


def load_ignores(pkgdirs):
    """Every `ignore` pattern the package carries, across all of its layers."""
    patterns = []
    for pkgdir in pkgdirs:
        path = os.path.join(pkgdir, NAME)
        if not os.path.isfile(path):
            continue
        for pattern in read_directives(path):
            if pattern not in patterns:
                patterns.append(pattern)
    return patterns


def literal(glob):
    """fnmatch reads [lua] as a character class; a VS Code key means it literally."""
    return glob.replace("[", "[[]")


def covers(path, pattern):
    segments = pattern.split("/")
    if not segments or len(segments) > len(path):
        return False
    return all(fnmatch.fnmatchcase(key, literal(glob)) for key, glob in zip(path, segments))


def matches(path, patterns):
    """True when a pattern names this key path or an ancestor of it."""
    return any(covers(path, pattern) for pattern in patterns)
