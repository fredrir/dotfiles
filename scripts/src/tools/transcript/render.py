import re

TOOL_HEAD_LINES = 20
TOOL_TAIL_LINES = 5
TOOL_LINE_LIMIT = 30


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


def render_turn(turn):
    if turn.kind == "me":
        return "> [!me] You\n" + prefix_quote(turn.body.strip())
    if turn.kind == "tool":
        header = f"> [!tool]- {clean_inline(turn.title, 90)}"
        body = cap_lines(turn.body.strip())
        if not body:
            return header
        fence = fence_for(body)
        return header + "\n" + prefix_quote(f"{fence}\n{body}\n{fence}")
    return "> [!turn]- Response\n" + prefix_quote(turn.body.strip())


def render_session(session):
    if session.degraded and not session.rounds:
        fence = fence_for(session.raw_text)
        return (
            "> [!agent]- Degraded import — format not recognized\n"
            + prefix_quote(f"{fence}\n{session.raw_text.strip()}\n{fence}")
        )
    parts = []
    used = {}
    for round_ in session.rounds:
        label = clean_inline(round_.label or "Response")
        if round_.timestamp is not None:
            heading = f"### {round_.timestamp:%H:%M} — {label}"
        else:
            heading = f"### {label}"
        count = used.get(heading, 0)
        used[heading] = count + 1
        if count:
            heading = f"{heading} ({count + 1})"
        parts.append(heading)
        parts.extend(render_turn(turn) for turn in round_.turns)
    return "\n\n".join(parts)


def render_capture(provider, stamp, text):
    header = f"> [!{provider}]- {stamp:%H:%M} clipboard capture"
    return header + "\n" + prefix_quote(text.strip())
