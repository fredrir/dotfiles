import re

from tools.transcript.model import Turn

TOOL_HEAD_LINES = 20
TOOL_TAIL_LINES = 5
TOOL_LINE_LIMIT = 30

TOC_MIN_ROUNDS = 5

PROVIDER_NAMES = {"claude": "Claude", "codex": "Codex"}


def prefix_quote(text):
    return "\n".join(("> " + line).rstrip() for line in text.split("\n"))


def fence_for(text):
    longest = 2
    for match in re.finditer(r"`+", text):
        longest = max(longest, len(match.group(0)))
    return "`" * (longest + 1)


def cap_lines(text, head=TOOL_HEAD_LINES, tail=TOOL_TAIL_LINES, limit=TOOL_LINE_LIMIT):
    lines = text.split("\n")
    if len(lines) <= limit:
        return text
    omitted = len(lines) - head - tail
    return "\n".join([*lines[:head], f"… (+{omitted} lines omitted)", *lines[-tail:]])


def clean_inline(text, limit=64):
    text = re.sub(r"[#>*`\[\]|{}]", "", " ".join(text.split())).strip()
    if len(text) > limit:
        text = text[: limit - 1].rstrip() + "…"
    return text


def render_turn(turn, provider):
    if turn.kind == "me":
        return "> [!me]+ You\n" + prefix_quote(turn.body.strip())
    if turn.kind == "tool":
        header = f"> [!tool]- {clean_inline(turn.title, 90)}"
        body = cap_lines(turn.body.strip())
        if not body:
            return header
        fence = fence_for(body)
        return header + "\n" + prefix_quote(f"{fence}\n{body}\n{fence}")
    name = PROVIDER_NAMES.get(provider, "Agent")
    return f"> [!turn|{provider}]- {name}\n" + prefix_quote(turn.body.strip())


def _round_turns(round_, include_tools):
    if include_tools:
        return round_.turns
    turns = [turn for turn in round_.turns if turn.kind == "me"]
    responses = [turn.body.strip() for turn in round_.turns if turn.kind == "turn" and turn.body.strip()]
    if responses:
        turns.append(Turn("turn", "Response", "\n\n".join(responses)))
    return turns


def render_session(session, include_tools=False):
    if session.degraded and not session.rounds:
        fence = fence_for(session.raw_text)
        return (
            "> [!agent]- Degraded import — format not recognized\n"
            + prefix_quote(f"{fence}\n{session.raw_text.strip()}\n{fence}")
        )
    headings = []
    blocks = []
    used = {}
    for round_ in session.rounds:
        turns = _round_turns(round_, include_tools)
        if not turns:
            continue
        label = clean_inline(round_.label or "Response")
        if round_.timestamp is not None:
            text = f"{round_.timestamp:%H:%M} — {label}"
        else:
            text = label
        count = used.get(text, 0)
        used[text] = count + 1
        if count:
            text = f"{text} ({count + 1})"
        headings.append(text)
        blocks.append(f"### {text}")
        blocks.extend(render_turn(turn, session.provider) for turn in turns)
    parts = []
    if len(headings) >= TOC_MIN_ROUNDS:
        toc_lines = "\n".join(f"> - [[#{text}|{text}]]" for text in headings)
        parts.append("> [!toc]- Contents\n" + toc_lines)
    parts.extend(blocks)
    return "\n\n".join(parts)


def render_capture(provider, stamp, text):
    header = f"> [!{provider}]- {stamp:%H:%M} clipboard capture"
    return header + "\n" + prefix_quote(text.strip())
