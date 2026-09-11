"""Format generated benchmark pins with dotfmt when available."""

import shutil

from tools.core.process import capture

PROGRAM = "dotfmt"


def formatted(text, name):
    """Return formatted text, retaining the original if dotfmt is unavailable or fails."""
    if not text or not shutil.which(PROGRAM):
        return text
    try:
        result = capture([PROGRAM, "--stdin", str(name)], input=text)
    except OSError:
        return text
    if result.returncode != 0 or not result.stdout:
        return text
    return result.stdout
