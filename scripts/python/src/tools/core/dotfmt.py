"""The one implementation of what a `.dotfile` file looks like.

Three generators write `.dotfile` files -- the package list, the benchmark
pins, and the theme selection -- and each used to align its own `=` column,
with three subtly different rules. They hand the text to `dotfmt` instead, so
the repository has a single answer to that question and `dotfmt` needs to know
nothing about the generators writing through it.
"""

import shutil

from tools.core.process import capture

PROGRAM = "dotfmt"


def formatted(text, name):
    """`text` as `dotfmt` would write it to `name`, or unchanged when it cannot.

    `setup.sh` builds `dotfmt`, and it runs the generators before it gets
    there, so an absent binary is an ordinary bootstrap state rather than an
    error: the file is written as generated and the next run aligns it. A
    formatter that fails is treated the same way -- a generator must not lose
    its output to a broken tool.
    """
    if not text or not shutil.which(PROGRAM):
        return text
    try:
        result = capture([PROGRAM, "--stdin", str(name)], input=text)
    except OSError:
        return text
    if result.returncode != 0 or not result.stdout:
        return text
    return result.stdout
