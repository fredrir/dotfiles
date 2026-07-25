from dataclasses import dataclass
import re


PROMPT_PATTERN = re.compile(
    r"^(?: \([^\n]*\))?(?:\[[^\n]*?\]){1,2}\$ ",
    re.MULTILINE,
)
SELECTOR_PATTERN = re.compile(
    r"^\s*(?P<value>[1-9]\d*|a|all)\s*(?:took\s+.+)?\s*$",
)


@dataclass(frozen=True)
class Selection:
    text: str
    count: int
    requested: str
    clear_input: bool


def _normalize(source: str) -> str:
    return source.replace("\r\n", "\n").replace("\r", "")


def _selector(source: str, prompt_end: int) -> tuple[str, bool]:
    current_line = source[prompt_end:].split("\n", 1)[0]
    match = SELECTOR_PATTERN.fullmatch(current_line)
    if match is None:
        return "1", False
    value = match.group("value")
    return ("all" if value in {"a", "all"} else value), True


def _amount(requested: str, available: int) -> int:
    if requested == "all":
        return available
    if not requested.isdecimal() or int(requested) < 1:
        raise ValueError("count must be a positive integer or all")
    return min(int(requested), available)


def select_blocks(source: str, requested: str | None = None) -> Selection:
    normalized = _normalize(source)
    prompts = list(PROMPT_PATTERN.finditer(normalized))
    if len(prompts) < 2:
        return Selection("", 0, requested or "1", False)

    clear_input = False
    if requested is None:
        requested, clear_input = _selector(normalized, prompts[-1].end())
    else:
        requested = requested.lower()
        if requested == "a":
            requested = "all"

    blocks = []
    for current, following in zip(prompts, prompts[1:]):
        block = normalized[current.start() : following.start()]
        if block.endswith("\n"):
            block = block[:-1]
        blocks.append(block)

    count = _amount(requested, len(blocks))
    return Selection("\n".join(blocks[-count:]), count, requested, clear_input)
