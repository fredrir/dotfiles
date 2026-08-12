import json
import os
import re

from tools.core.process import capture
from tools.dotfile.secret.identity import identity_path, sops_env

FACTS = "facts.enc.yaml"
OPEN = "open."
PLACEHOLDER = re.compile(r"\{\{\s*([A-Za-z0-9_][A-Za-z0-9_.-]*)\s*\}\}")


class Facts:
    def __init__(self, values, ok, note):
        self.values = values
        self.ok = ok
        self.note = note


def facts_file(ctx):
    return os.path.join(ctx.root, FACTS)


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
            return f"'{name}' is a list; facts must be single values"
        elif value is None:
            return f"'{name}' has no value"
        else:
            out[name] = scalar(value)
    return ""


def load(ctx):
    path = facts_file(ctx)
    if not os.path.isfile(path):
        return Facts({}, True, "")
    if not os.path.isfile(identity_path(ctx)):
        return Facts({}, False, f"{FACTS} needs an age identity to read")
    result = capture(["sops", "-d", "--output-type", "json", path], env=sops_env(ctx))
    if result.returncode != 0:
        return Facts({}, False, f"{FACTS} did not decrypt on this machine")
    try:
        raw = json.loads(result.stdout)
    except ValueError:
        return Facts({}, False, f"{FACTS} did not decrypt to an object")
    if not isinstance(raw, dict):
        return Facts({}, False, f"{FACTS} must hold a mapping")
    values = {}
    problem = flatten(raw, "", values)
    if problem:
        return Facts({}, False, problem)
    return Facts(values, True, "")


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
