"""Surgical single-key edits to a repo-tracked JSONC file.

Only the bytes of the member being touched change: comments, blank lines, the file's
indent style and every other member survive verbatim. Paths are tuples of literal keys,
so ("editor.formatOnSave",) is one flat top-level key.
"""

import json
import os

from tools.dotfile import jsonspan


def set_key(path_to_file, path, value):
    """Insert or replace path's member in the file, preserving every other byte
    including comments and the file's existing indent style. Creates the file
    (as a minimal `{}` document) if it does not exist."""
    if not path:
        raise ValueError("set_key needs at least one key")
    text = _read(path_to_file)
    _write(path_to_file, apply_set(text, path, value))


def remove_key(path_to_file, path):
    """Delete path's member, leaving surrounding bytes and comma structure valid.
    Returns False and touches nothing when the file or the key is absent."""
    if not path:
        raise ValueError("remove_key needs at least one key")
    if not os.path.isfile(path_to_file):
        return False
    text = _read(path_to_file)
    cut = apply_remove(text, path)
    if cut is None:
        return False
    _write(path_to_file, cut)
    return True


def apply_set(text, path, value):
    """The document text with path's member inserted, or its value replaced in place."""
    unit = jsonspan.detect_indent(text)
    nl = _newline(text)
    span = jsonspan.value_span(text, path)
    if span is not None:
        start, end = span
        rendered = _render(value, unit, _line_indent(text, start), nl)
        return text[:start] + rendered + text[end:]
    body, missing = _target(text, path)
    return _insert(text, body, missing[0], _nest(missing[1:], value), unit, nl)


def apply_remove(text, path):
    """The document text with path's member deleted, or None when it is not there."""
    body = jsonspan.container_span(text, path)
    if body is None:
        return None
    items = jsonspan.members(text, body)
    index = None
    for position, item in enumerate(items):
        if item[0] == path[-1]:
            index = position
    if index is None:
        return None
    start, end = items[index][1], items[index][3]
    comma = None
    probe = jsonspan.skip_blanks(text, end)
    if probe < body[1] and text[probe] == ",":
        end = probe + 1
    elif index:
        prior = jsonspan.skip_blanks(text, items[index - 1][3])
        if text[prior] == ",":
            comma = prior
    head = text.rfind("\n", 0, start) + 1
    if not text[head:start].strip():
        start = head
    line = text.find("\n", end)
    if line != -1:
        rest = text[end:line]
        if not rest.strip() or rest.lstrip().startswith("//"):
            end = line + 1
    if comma is None:
        return text[:start] + text[end:]
    return text[:comma] + text[comma + 1 : start] + text[end:]


def _target(text, path):
    """(body span to insert into, the tail of path that still has to be created)."""
    body = jsonspan.container_span(text, path[:1])
    if body is None:
        raise ValueError("the document root is not a JSON object")
    for depth in range(len(path) - 1):
        inner = jsonspan.container_span(text, path[: depth + 2])
        if inner is None:
            if jsonspan.key_span(text, path[: depth + 1]) is not None:
                raise ValueError(f"'{path[depth]}' is not an object")
            return body, path[depth:]
        body = inner
    return body, path[-1:]


def _nest(keys, value):
    """Wrap value in one fresh object per missing intermediate key."""
    for key in reversed(keys):
        value = {key: value}
    return value


def _insert(text, body, key, value, unit, nl):
    """Add a member at the end of a container, giving the previous one its comma."""
    end = body[1]
    items = jsonspan.members(text, body)
    if not items:
        return _insert_first(text, body, key, value, unit, nl)
    tail = items[-1][3]
    probe = jsonspan.skip_blanks(text, tail)
    comma = probe < end and text[probe] == ","
    lead = "" if comma else ","
    if comma:
        tail = probe + 1
    cut = text.find("\n", tail)
    if cut == -1 or cut > end:
        return text[:tail] + lead + " " + _flat(key, value) + text[tail:]
    if text[cut - 1] == "\r":
        cut -= 1
    indent = _line_indent(text, items[-1][1])
    member = _member(key, value, unit, indent, nl)
    return text[:tail] + lead + text[tail:cut] + nl + indent + member + text[cut:]


def _insert_first(text, body, key, value, unit, nl):
    """Add the only member of a container that holds no members yet."""
    start, end = body
    outer = _line_indent(text, end)
    indent = outer + unit
    member = _member(key, value, unit, indent, nl)
    if "\n" in text[start:end]:
        at = text.rfind("\n", 0, end) + 1
        return text[:at] + indent + member + nl + text[at:]
    return text[:end] + nl + indent + member + nl + outer + text[end:]


def _render(value, unit, indent, nl):
    """The value as JSON: nested structure uses unit and hangs under indent."""
    return json.dumps(value, indent=unit, ensure_ascii=False).replace("\n", nl + indent)


def _member(key, value, unit, indent, nl):
    return json.dumps(key, ensure_ascii=False) + ": " + _render(value, unit, indent, nl)


def _flat(key, value):
    """A member on one line, for a container that keeps everything on one line."""
    return json.dumps(key, ensure_ascii=False) + ": " + json.dumps(value, ensure_ascii=False)


def _newline(text):
    """The file's line ending: CRLF when its first line ends that way, else LF."""
    cut = text.find("\n")
    if cut > 0 and text[cut - 1] == "\r":
        return "\r\n"
    return "\n"


def _line_indent(text, offset):
    """The leading whitespace of the line that holds offset."""
    start = text.rfind("\n", 0, offset) + 1
    probe = start
    while probe < len(text) and text[probe] in " \t":
        probe += 1
    return text[start:probe]


def _read(path_to_file):
    if not os.path.isfile(path_to_file):
        return "{}\n"
    with open(path_to_file, encoding="utf-8", newline="") as handle:
        return handle.read()


def _write(path_to_file, text):
    parent = os.path.dirname(path_to_file)
    if parent:
        os.makedirs(parent, exist_ok=True)
    with open(path_to_file, "w", encoding="utf-8", newline="") as handle:
        handle.write(text)
