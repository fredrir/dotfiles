import os
import tempfile

from tools.theme.model import ROOT


def write_atomic(target, content):
    directory = os.path.dirname(target)
    os.makedirs(directory, exist_ok=True)
    try:
        mode = os.stat(target).st_mode & 0o7777
    except FileNotFoundError:
        mode = 0o644 & ~_umask()
    descriptor, temporary = tempfile.mkstemp(dir=directory, prefix=".dotfile-theme.")
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(content)
        os.chmod(temporary, mode)
        os.replace(temporary, target)
    except BaseException:
        os.unlink(temporary)
        raise


def _umask():
    current = os.umask(0)
    os.umask(current)
    return current


class Output:
    def __init__(self, dry=False):
        self.dry = dry
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
        if not self.dry:
            write_atomic(target, content)
        self._record(target)

    def edit(self, target, transform):
        try:
            with open(target, encoding="utf-8") as handle:
                previous = handle.read()
        except FileNotFoundError:
            raise SystemExit(f"dotfile theme: {os.path.relpath(target, ROOT)} is missing")
        updated = transform(previous)
        if updated == previous:
            return
        if not self.dry:
            write_atomic(target, updated)
        self._record(target)


class ScopedOutput:
    def __init__(self, output, allowed):
        self._output = output
        self._allowed = {os.path.abspath(target) for target in allowed}

    def _permits(self, target):
        return os.path.abspath(target) in self._allowed

    def write(self, target, content):
        if self._permits(target):
            self._output.write(target, content)

    def edit(self, target, transform):
        if self._permits(target):
            self._output.edit(target, transform)


def replace_between(text, name, new_lines, indent=None):
    """Swap the block between two markers, indented the way the marker is."""
    start = f"theme:{name}"
    end = f"theme:{name}:end"
    lines = text.split("\n")
    first = next((i for i, line in enumerate(lines) if end not in line and start in line), None)
    last = next((i for i, line in enumerate(lines) if end in line), None)
    if first is None or last is None or last < first:
        raise SystemExit(f"markers '{start}' / '{end}' not found in the target file")
    if indent is None:
        indent = lines[first][: len(lines[first]) - len(lines[first].lstrip())]
    body = [indent + line if line else line for line in new_lines]
    return "\n".join(lines[: first + 1] + body + lines[last:])


def _ini_section_bounds(lines, header, where):
    marker = f"[{header}]"
    start = next((i for i, line in enumerate(lines) if line == marker), None)
    if start is None:
        raise SystemExit(f"{where}: section '{marker}' not found")
    end = start + 1
    while end < len(lines) and not lines[end].startswith("["):
        end += 1
    return start, end


def replace_ini_section(text, header, body_lines, where="kdeglobals"):
    lines = text.split("\n")
    start, end = _ini_section_bounds(lines, header, where)
    old_body = lines[start + 1 : end]
    blanks = 0
    while old_body and old_body[-1] == "":
        blanks += 1
        old_body.pop()
    new_body = list(body_lines) + [""] * blanks
    return "\n".join(lines[: start + 1] + new_body + lines[end:])


def get_ini_key(text, header, key, where="config"):
    lines = text.split("\n")
    start, end = _ini_section_bounds(lines, header, where)
    for index in range(start + 1, end):
        name, sep, value = lines[index].partition("=")
        if sep and name == key:
            return value
    return None


def set_ini_key(text, header, key, value, where="kdeglobals"):
    lines = text.split("\n")
    start, end = _ini_section_bounds(lines, header, where)
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
