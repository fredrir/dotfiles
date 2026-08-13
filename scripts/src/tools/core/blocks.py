from dataclasses import dataclass

UNEXPECTED_CLOSE = "unexpected-close"
NESTED = "nested"
OUTSIDE = "outside"
UNTERMINATED = "unterminated"


class BlockError(Exception):
    def __init__(self, kind, number, block=""):
        super().__init__(kind)
        self.kind = kind
        self.number = number
        self.block = block


@dataclass(frozen=True)
class Entry:
    block: str
    number: int
    text: str
    opens: bool = False

    def split(self, separator="="):
        key, found, value = self.text.partition(separator)
        if not found:
            return trim(self.text), ""
        return trim(key), trim(value)

    def fields(self):
        return self.text.split()


def trim(value):
    return value.strip(" \t\n\r\f\v")


def scan(lines, comments=True, open_suffix="{"):
    entries = []
    block = ""
    number = 0
    for raw in lines:
        number += 1
        line = trim(raw.split("#", 1)[0] if comments else raw)
        if not line:
            continue
        if line == "}":
            if not block:
                raise BlockError(UNEXPECTED_CLOSE, number)
            block = ""
            continue
        if line.endswith(open_suffix):
            if block:
                raise BlockError(NESTED, number)
            block = trim(line[: -len(open_suffix)])
            entries.append(Entry(block, number, "", opens=True))
            continue
        if not block:
            raise BlockError(OUTSIDE, number)
        entries.append(Entry(block, number, line))
    if block:
        raise BlockError(UNTERMINATED, number, block)
    return entries


def read(path, comments=True, open_suffix="{"):
    try:
        with open(path, encoding="utf-8") as handle:
            lines = handle.read().splitlines()
    except OSError:
        return []
    return scan(lines, comments=comments, open_suffix=open_suffix)
