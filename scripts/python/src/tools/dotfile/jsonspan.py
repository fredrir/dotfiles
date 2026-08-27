"""Byte spans inside JSONC text, so one key can be edited without reformatting the rest.

A path is a tuple of literal key strings. A dot inside a key belongs to that key and is
never a nesting separator: ("editor.formatOnSave",) is one top-level key, while
("[lua]", "editor.tabSize") is two levels deep. Duplicate keys resolve to the last one,
matching JSON.parse.
"""

import json


def skip_blanks(text, index):
    """Advance past whitespace, `//` line comments and `/* */` block comments."""
    size = len(text)
    while index < size:
        char = text[index]
        if char in " \t\r\n":
            index += 1
            continue
        if text.startswith("//", index):
            end = text.find("\n", index)
            index = size if end == -1 else end
            continue
        if text.startswith("/*", index):
            end = text.find("*/", index + 2)
            if end == -1:
                raise ValueError("unterminated block comment")
            index = end + 2
            continue
        break
    return index


def _scan_string(text, index):
    """(decoded value, offset just past the closing quote) for the string at index."""
    size = len(text)
    probe = index + 1
    while probe < size:
        char = text[probe]
        if char == "\\":
            probe += 2
            continue
        if char == '"':
            return json.loads(text[index : probe + 1]), probe + 1
        probe += 1
    raise ValueError(f"unterminated string at offset {index}")


def _scan_container(text, index):
    """The offset just past the object or array whose bracket sits at index."""
    close = "}" if text[index] == "{" else "]"
    size = len(text)
    depth = 0
    while index < size:
        char = text[index]
        if char == '"':
            _value, index = _scan_string(text, index)
            continue
        if text.startswith("//", index) or text.startswith("/*", index):
            index = skip_blanks(text, index)
            continue
        if char in "{[":
            depth += 1
        elif char in "}]":
            depth -= 1
            if depth == 0:
                if char != close:
                    raise ValueError(f"mismatched bracket at offset {index}")
                return index + 1
        index += 1
    raise ValueError("unterminated container")


def _scan_value(text, index):
    """The offset just past the value starting at index."""
    char = text[index]
    if char == '"':
        return _scan_string(text, index)[1]
    if char in "{[":
        return _scan_container(text, index)
    size = index
    while size < len(text) and (text[size].isalnum() or text[size] in "+-."):
        size += 1
    return size


def _root_body(text):
    """The body span of the top-level object, or None when the document is not one."""
    index = skip_blanks(text, 0)
    if index >= len(text) or text[index] != "{":
        return None
    return index + 1, _scan_container(text, index) - 1


def members(text, body):
    """[(key, key start, value start, value end)] for every member of an object body."""
    start, end = body
    found = []
    index = start
    while True:
        index = skip_blanks(text, index)
        if index >= end:
            return found
        if text[index] == ",":
            index += 1
            continue
        if text[index] != '"':
            raise ValueError(f"expected a key at offset {index}")
        key, after = _scan_string(text, index)
        after = skip_blanks(text, after)
        if after >= end or text[after] != ":":
            raise ValueError(f"expected ':' after the key at offset {index}")
        value_start = skip_blanks(text, after + 1)
        value_end = _scan_value(text, value_start)
        if value_end == value_start:
            raise ValueError(f"expected a value at offset {value_start}")
        found.append((key, index, value_start, value_end))
        index = value_end


def _find(text, body, key):
    """(key start, value start, value end) of the last member named key, or None."""
    found = None
    for name, key_start, value_start, value_end in members(text, body):
        if name == key:
            found = (key_start, value_start, value_end)
    return found


def _descend(text, path):
    """The body span of the object at path, or None if it is missing or not an object."""
    body = _root_body(text)
    for key in path:
        if body is None:
            return None
        found = _find(text, body, key)
        if found is None:
            return None
        _key_start, value_start, value_end = found
        if text[value_start] != "{":
            return None
        body = (value_start + 1, value_end - 1)
    return body


def key_span(text, path):
    """(start, end) offsets of the `"key": value` member at path, or None.
    start = offset of the key's opening quote; end = offset just past the value."""
    if not path:
        return None
    body = _descend(text, path[:-1])
    if body is None:
        return None
    found = _find(text, body, path[-1])
    if found is None:
        return None
    return found[0], found[2]


def value_span(text, path):
    """(start, end) offsets of just the value at path, or None. The key, the colon and
    any comment trailing the member sit outside this span."""
    if not path:
        return None
    body = _descend(text, path[:-1])
    if body is None:
        return None
    found = _find(text, body, path[-1])
    if found is None:
        return None
    return found[1], found[2]


def container_span(text, path):
    """(start, end) offsets of the body of the object that would contain path
    (i.e. just inside its braces), or None if an ancestor is missing or not an object."""
    if not path:
        return None
    return _descend(text, path[:-1])


def detect_indent(text):
    """The file's indent unit: a tab or N spaces, taken from the first indented line.
    Defaults to a tab when the file has no indented line."""
    for line in text.splitlines():
        if not line.strip():
            continue
        if line.startswith("\t"):
            return "\t"
        if line.startswith(" "):
            return " " * (len(line) - len(line.lstrip(" ")))
    return "\t"
