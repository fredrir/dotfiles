import json
import os
import re

from tools.core.process import capture
from tools.dotfile.secret.identity import identity_path, sops_env

VARS = "vars.enc.yaml"
OPEN = "open."
PLACEHOLDER = re.compile(r"\{\{\s*([A-Za-z0-9_][A-Za-z0-9_.-]*)\s*\}\}")


class Vars:
    def __init__(self, values, ok, note):
        self.values = values
        self.ok = ok
        self.note = note


def vars_file(ctx):
    return os.path.join(ctx.root, VARS)


def scalar(value):
    if isinstance(value, bool):
        return "true" if value else "false"
    return str(value)


def flatten(node, prefix, out):
    for key, value in node.items():
        name = f"{prefix}.{key}" if prefix else key
        if isinstance(value, dict):
            problem = flatten(value, name, out)
            if problem:
                return problem
        elif isinstance(value, list):
            return f"'{name}' is a list; a var must be a single value"
        elif value is None:
            return f"'{name}' has no value"
        else:
            out[name] = scalar(value)
    return ""


def load(ctx):
    path = vars_file(ctx)
    if not os.path.isfile(path):
        return Vars({}, True, "")
    if not os.path.isfile(identity_path(ctx)):
        return Vars({}, False, f"{VARS} needs an age identity to read")
    result = capture(["sops", "-d", "--output-type", "json", path], env=sops_env(ctx))
    if result.returncode != 0:
        return Vars({}, False, f"{VARS} did not decrypt on this machine")
    try:
        raw = json.loads(result.stdout)
    except ValueError:
        return Vars({}, False, f"{VARS} did not decrypt to an object")
    if not isinstance(raw, dict):
        return Vars({}, False, f"{VARS} must hold a mapping")
    values = {}
    problem = flatten(raw, "", values)
    if problem:
        return Vars({}, False, problem)
    return Vars(values, True, "")


def references(text):
    return sorted({match.group(1) for match in PLACEHOLDER.finditer(text)})


def render(text, values):
    missing = []

    def substitute(match):
        name = match.group(1)
        if name in values:
            return values[name]
        missing.append(name)
        return match.group(0)

    rendered = PLACEHOLDER.sub(substitute, text)
    return rendered, sorted(set(missing))
