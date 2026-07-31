import os

from .model import ROOT


class Output:
    def __init__(self, check=False):
        self.check = check
        self.changed = []

    def _record(self, target):
        self.changed.append(os.path.relpath(target, ROOT))

    def write(self, target, content):
        previous = None
        if os.path.exists(target):
            with open(target, encoding="utf-8") as handle:
                previous = handle.read()
        if previous == content:
            return
        if not self.check:
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with open(target, "w", encoding="utf-8") as handle:
                handle.write(content)
        self._record(target)

    def edit(self, target, transform):
        with open(target, encoding="utf-8") as handle:
            previous = handle.read()
        updated = transform(previous)
        if updated == previous:
            return
        if not self.check:
            with open(target, "w", encoding="utf-8") as handle:
                handle.write(updated)
        self._record(target)


def replace_between(text, name, new_lines, indent=""):
    start = f"theme:{name}"
    end = f"theme:{name}:end"
    lines = text.split("\n")
    first = next((i for i, line in enumerate(lines) if end not in line and start in line), None)
    last = next((i for i, line in enumerate(lines) if end in line), None)
    if first is None or last is None or last < first:
        raise SystemExit(f"markers '{start}' / '{end}' not found in the target file")
    body = [indent + line if line else line for line in new_lines]
    return "\n".join(lines[: first + 1] + body + lines[last:])


def _ini_section_bounds(lines, header):
    marker = f"[{header}]"
    start = next((i for i, line in enumerate(lines) if line == marker), None)
    if start is None:
        raise SystemExit(f"kdeglobals: section '{marker}' not found")
    end = start + 1
    while end < len(lines) and not lines[end].startswith("["):
        end += 1
    return start, end


def replace_ini_section(text, header, body_lines):
    lines = text.split("\n")
    start, end = _ini_section_bounds(lines, header)
    old_body = lines[start + 1:end]
    blanks = 0
    while old_body and old_body[-1] == "":
        blanks += 1
        old_body.pop()
    new_body = list(body_lines) + [""] * blanks
    return "\n".join(lines[:start + 1] + new_body + lines[end:])


def set_ini_key(text, header, key, value):
    lines = text.split("\n")
    start, end = _ini_section_bounds(lines, header)
    for index in range(start + 1, end):
        if lines[index].split("=", 1)[0] == key:
            lines[index] = f"{key}={value}"
            return "\n".join(lines)
    insert_at = end
    for index in range(start + 1, end):
        name = lines[index].split("=", 1)[0]
        if name and not lines[index].startswith("[") and name > key:
            insert_at = index
            break
    lines.insert(insert_at, f"{key}={value}")
    return "\n".join(lines)
