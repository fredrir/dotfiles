from dataclasses import dataclass

UNEXPECTED_CLOSE = "unexpected-close"
NESTED = "nested"
OUTSIDE = "outside"
UNTERMINATED = "unterminated"

# How '#' is read. True strips it anywhere, which loses part numbers and
# revisions that legitimately contain one. LINE strips only whole-line comments,
# so a file can carry a header and still hold '#' inside a value. False is for
# files that are generated rather than written by hand.
LINE = "line"


STRUCTURE_ERRORS = {
    UNEXPECTED_CLOSE: "unexpected }}",
    NESTED: "nested {noun}",
    OUTSIDE: "entry outside a {noun}",
    UNTERMINATED: "missing }} for {block}",
}


class BlockError(Exception):
    def __init__(self, kind, number, block=""):
        super().__init__(kind)
        self.kind = kind
        self.number = number
        self.block = block


def describe(error, label, noun="block"):
    """Render a BlockError as one line a person can act on.

    Every caller used to keep its own copy of this table, and four of them
    forgot to catch the error at all -- so a stray brace in a config file
    surfaced as a traceback. Owning the wording here is what makes that
    mistake hard to write.
    """
    text = STRUCTURE_ERRORS[error.kind].format(noun=noun, block=error.block)
    return f"{label}:{error.number}: {text}"


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


def uncomment(raw, comments):
    line = trim(raw)
    if not comments:
        return line
    if comments == LINE:
        return "" if line.startswith("#") else line
    return trim(line.split("#", 1)[0])


def scan(lines, comments=True, open_suffix="{"):
    entries = []
    block = ""
    number = 0
    for raw in lines:
        number += 1
        line = uncomment(raw, comments)
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
