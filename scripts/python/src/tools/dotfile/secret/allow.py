import fnmatch
import os

from tools.core import blocks
from tools.dotfile.state import die


def allow_file(ctx):
    return os.path.join(ctx.root, "config/scan.dotfile")


def load_allow(ctx):
    rules = []
    path = allow_file(ctx)
    if not os.path.isfile(path):
        return rules
    try:
        entries = blocks.read(path)
    except blocks.BlockError as error:
        die(blocks.describe(error, "config/scan.dotfile", "'allow {' block"))
        return rules
    for entry in entries:
        if entry.opens:
            if entry.block != "allow":
                die(f"config/scan.dotfile:{entry.number}: unknown block '{entry.block}'")
            continue
        fields = entry.fields()
        rules.append((fields[0], fields[1] if len(fields) > 1 else ""))
    return rules


def allowed(rules, path, label):
    for glob, only in rules:
        if only and only != label:
            continue
        if path == glob or fnmatch.fnmatch(path, glob):
            return True
    return False
