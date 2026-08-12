import fnmatch
import os

from tools.dotfile.state import die, trim


def allow_file(ctx):
    return os.path.join(ctx.root, "scan.dotfile")


def load_allow(ctx):
    rules = []
    path = allow_file(ctx)
    if not os.path.isfile(path):
        return rules
    with open(path, encoding="utf-8") as handle:
        lines = handle.read().splitlines()
    block = ""
    number = 0
    for raw in lines:
        number += 1
        line = trim(raw.split("#", 1)[0])
        if not line:
            continue
        if line == "}":
            if not block:
                die(f"scan.dotfile:{number}: unexpected }}")
            block = ""
            continue
        if line.endswith("{"):
            if block:
                die(f"scan.dotfile:{number}: nested block")
            block = trim(line[:-1])
            if block != "allow":
                die(f"scan.dotfile:{number}: unknown block '{block}'")
            continue
        if not block:
            die(f"scan.dotfile:{number}: line outside an 'allow {{' block")
        fields = line.split()
        rules.append((fields[0], fields[1] if len(fields) > 1 else ""))
    if block:
        die("scan.dotfile: unterminated block")
    return rules


def allowed(rules, path, label):
    for glob, only in rules:
        if only and only != label:
            continue
        if path == glob or fnmatch.fnmatch(path, glob):
            return True
    return False
