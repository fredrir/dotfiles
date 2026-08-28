"""JSONC reading for VS Code-style files: comments and trailing commas allowed."""

import json


def strip_comments(text):
    out = []
    index = 0
    size = len(text)
    in_string = False
    while index < size:
        char = text[index]
        if in_string:
            out.append(char)
            if char == "\\" and index + 1 < size:
                out.append(text[index + 1])
                index += 2
                continue
            if char == '"':
                in_string = False
            index += 1
            continue
        if char == '"':
            in_string = True
            out.append(char)
            index += 1
            continue
        if text.startswith("//", index):
            end = text.find("\n", index)
            if end == -1:
                break
            index = end
            continue
        if text.startswith("/*", index):
            end = text.find("*/", index + 2)
            if end == -1:
                raise ValueError("unterminated block comment")
            index = end + 2
            continue
        out.append(char)
        index += 1
    return "".join(out)


def strip_trailing_commas(text):
    out = []
    index = 0
    size = len(text)
    in_string = False
    while index < size:
        char = text[index]
        if in_string:
            out.append(char)
            if char == "\\" and index + 1 < size:
                out.append(text[index + 1])
                index += 2
                continue
            if char == '"':
                in_string = False
            index += 1
            continue
        if char == '"':
            in_string = True
            out.append(char)
            index += 1
            continue
        if char == ",":
            probe = index + 1
            while probe < size and text[probe] in " \t\r\n":
                probe += 1
            if probe < size and text[probe] in "}]":
                index += 1
                continue
        out.append(char)
        index += 1
    return "".join(out)


def loads(text):
    """Parse JSONC: `//` and `/* */` comments and trailing commas are removed first."""
    return json.loads(strip_trailing_commas(strip_comments(text)))
