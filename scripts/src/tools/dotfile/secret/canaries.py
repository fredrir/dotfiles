import os
import stat

from tools.core.patterns import mask
from tools.dotfile.secret.variables import OPEN
from tools.dotfile.secret.variables import load as load_vars
from tools.dotfile.state import trim

MIN_LENGTH = 6


class Canary:
    def __init__(self, label, value):
        self.label = label
        self.value = value
        self.needle = value.lower()


def canaries_file(ctx):
    return os.path.join(ctx.state_dir, "canaries")


def mode_note(path):
    try:
        mode = stat.S_IMODE(os.stat(path).st_mode)
    except OSError:
        return ""
    if mode & 0o077:
        return f"canaries file is readable beyond you: chmod 600 {path}"
    return ""


def load_canaries(ctx):
    found = []
    notes = []
    path = canaries_file(ctx)
    if not os.path.isfile(path):
        return found, notes
    note = mode_note(path)
    if note:
        notes.append(note)
    with open(path, encoding="utf-8") as handle:
        lines = handle.read().splitlines()
    for raw in lines:
        line = trim(raw.split("#", 1)[0])
        if not line:
            continue
        if "=" in line:
            label, _, value = line.partition("=")
            label, value = trim(label), trim(value)
        else:
            label, value = "", line
        if not value:
            continue
        label = label or mask(value)
        if len(value) < MIN_LENGTH:
            notes.append(f"canary too short to match usefully: {label}")
            continue
        found.append(Canary(label, value))
    return found, notes


def all_canaries(ctx):
    found, notes = load_canaries(ctx)
    declared = load_vars(ctx)
    if declared.note:
        notes.append(declared.note)
    seen = {canary.needle for canary in found}
    for name, value in sorted(declared.values.items()):
        if name.startswith(OPEN) or len(value) < MIN_LENGTH or value.lower() in seen:
            continue
        seen.add(value.lower())
        found.append(Canary(name, value))
    return found, notes


def private_values(ctx):
    found, notes = all_canaries(ctx)
    return [canary.value for canary in found], notes
