import os

from tools.core import blocks
from tools.theme.model import ROOT, list_profiles
from tools.theme.render import write_atomic

SELECTION_FILE = os.path.join(ROOT, "profiles.dotfile")

DEFAULT_GROUP = "shared"
THEME_KEY = "theme"

MESSAGES = {
    blocks.UNEXPECTED_CLOSE: "unexpected }",
    blocks.NESTED: "nested group",
    blocks.OUTSIDE: "entry outside a group",
    blocks.UNTERMINATED: "unterminated group",
}


def group_of(relpath):
    parts = relpath.split("/")
    if parts[0] == "linux" and len(parts) > 1:
        return "/".join(parts[:2])
    return parts[0]


def package_of(relpath):
    group = group_of(relpath)
    remainder = relpath[len(group) + 1 :]
    return remainder.split("/")[0]


def inventory(owned):
    packages = {}
    for relpath in owned:
        packages.setdefault(group_of(relpath), set()).add(package_of(relpath))
    return {group: sorted(packages[group]) for group in sorted(packages)}


class Selection:
    def __init__(self, groups):
        self.groups = groups

    @property
    def default(self):
        return self.groups[DEFAULT_GROUP][THEME_KEY]

    def for_path(self, relpath):
        group = self.groups.get(group_of(relpath), {})
        package = package_of(relpath)
        if package in group:
            return group[package]
        return group.get(THEME_KEY, self.default)

    def assignments(self, relpaths):
        used = {}
        for relpath in relpaths:
            used.setdefault(self.for_path(relpath), []).append(relpath)
        return used

    def scope_of(self, relpath):
        group = group_of(relpath)
        package = package_of(relpath)
        if package in self.groups.get(group, {}):
            return f"{group}/{package}"
        return group

    def scopes(self, relpaths):
        found = {}
        for relpath in relpaths:
            found.setdefault(self.for_path(relpath), set()).add(self.scope_of(relpath))
        return {name: sorted(found[name]) for name in found}

    def entries(self):
        return [(group, key) for group in sorted(self.groups) for key in sorted(self.groups[group])]

    def overrides(self):
        return [
            (group, key)
            for group, key in self.entries()
            if (group, key) != (DEFAULT_GROUP, THEME_KEY)
        ]


def _fail(problems):
    listed = "\n".join(f"  {problem}" for problem in problems)
    raise SystemExit(f"dotfile theme: profiles.dotfile is not usable:\n{listed}")


def read_selection(owned):
    try:
        entries = blocks.scan(_lines())
    except blocks.BlockError as error:
        reason = MESSAGES.get(error.kind, error.kind)
        raise SystemExit(f"dotfile theme: profiles.dotfile line {error.number}: {reason}")

    groups = {}
    problems = []
    available = set(list_profiles())
    packages = {}
    for relpath in owned:
        packages.setdefault(group_of(relpath), set()).add(package_of(relpath))

    for entry in entries:
        groups.setdefault(entry.block, {})
        if entry.opens:
            continue
        key, value = entry.split()
        value = value.strip("\"'")
        if not value:
            problems.append(f"line {entry.number}: '{key}' has no profile")
            continue
        if value not in available:
            listed = ", ".join(sorted(available))
            problems.append(f"line {entry.number}: unknown profile '{value}' (available: {listed})")
            continue
        groups[entry.block][key] = value

    for group, keys in groups.items():
        if group not in packages:
            listed = ", ".join(sorted(packages))
            problems.append(f"group '{group}' owns no generated file (groups: {listed})")
            continue
        for key in keys:
            if key != THEME_KEY and key not in packages[group]:
                listed = ", ".join(sorted(packages[group]))
                problems.append(
                    f"group '{group}' has no '{key}' output to theme (owns: {listed})"
                )

    if not problems and THEME_KEY not in groups.get(DEFAULT_GROUP, {}):
        problems.append(f"'{DEFAULT_GROUP}' must set a '{THEME_KEY}', it is the fallback")

    if problems:
        _fail(problems)
    return Selection(groups)


def default_profile():
    try:
        entries = blocks.scan(_lines())
    except blocks.BlockError as error:
        reason = MESSAGES.get(error.kind, error.kind)
        raise SystemExit(f"dotfile theme: profiles.dotfile line {error.number}: {reason}")
    for entry in entries:
        if entry.opens or entry.block != DEFAULT_GROUP:
            continue
        key, value = entry.split()
        if key == THEME_KEY and value:
            return value
    raise SystemExit(
        f"dotfile theme: profiles.dotfile has no '{THEME_KEY}' under '{DEFAULT_GROUP}'"
    )


def _lines():
    try:
        with open(SELECTION_FILE, encoding="utf-8") as handle:
            return handle.read().splitlines()
    except FileNotFoundError:
        raise SystemExit(
            "dotfile theme: profiles.dotfile is missing, cannot tell which profiles to use"
        )


def _code(line):
    return blocks.trim(line.split("#", 1)[0])


def _spans(lines):
    found = {}
    name = ""
    start = 0
    for index, line in enumerate(lines):
        body = _code(line)
        if not body:
            continue
        if body == "}":
            if name:
                found[name] = (start, index)
                name = ""
        elif body.endswith("{"):
            name = blocks.trim(body[:-1])
            start = index
    return found


def _entry_lines(lines, span):
    start, end = span
    return [index for index in range(start + 1, end) if _code(lines[index])]


def _key_of(line):
    return blocks.trim(_code(line).split("=", 1)[0])


def _comment_of(line):
    position = line.find("#")
    return (line, "") if position < 0 else (line[:position], line[position:])


def _retarget(line, value):
    body, comment = _comment_of(line)
    head, separator, tail = body.partition("=")
    lead = tail[: len(tail) - len(tail.lstrip())] or " "
    trail = tail[len(tail.rstrip()) :]
    return f"{head}{separator}{lead}{value}{trail}{comment}"


def _realigned(lines, span, indent):
    entries = _entry_lines(lines, span)
    width = max((len(_key_of(lines[index])) for index in entries), default=0)
    for index in entries:
        body, comment = _comment_of(lines[index])
        key, _separator, tail = body.partition("=")
        lines[index] = f"{indent}{blocks.trim(key).ljust(width)} = {blocks.trim(tail)}{comment}"


def _indent_of(lines, span):
    for index in _entry_lines(lines, span):
        raw = lines[index]
        return raw[: len(raw) - len(raw.lstrip())] or "  "
    return "  "


def _save(lines):
    write_atomic(SELECTION_FILE, "\n".join(lines) + "\n")


def assign(block, key, value):
    lines = _lines()
    spans = _spans(lines)
    if block not in spans:
        prefix = [""] if lines and lines[-1].strip() else []
        _save(lines + prefix + [f"{block} {{", f"  {key} = {value}", "}"])
        return True
    span = spans[block]
    for index in _entry_lines(lines, span):
        if _key_of(lines[index]) != key:
            continue
        updated = _retarget(lines[index], value)
        if updated == lines[index]:
            return False
        lines[index] = updated
        _save(lines)
        return True
    indent = _indent_of(lines, span)
    lines.insert(span[1], f"{indent}{key} = {value}")
    _realigned(lines, (span[0], span[1] + 1), indent)
    _save(lines)
    return True


def unassign(block, key):
    lines = _lines()
    spans = _spans(lines)
    if block not in spans:
        return False
    start, end = spans[block]
    entries = _entry_lines(lines, (start, end))
    found = [index for index in entries if _key_of(lines[index]) == key]
    if not found:
        return False
    if len(entries) > 1:
        del lines[found[0]]
    else:
        while start and not lines[start - 1].strip():
            start -= 1
        del lines[start : end + 1]
    _save(lines)
    return True
