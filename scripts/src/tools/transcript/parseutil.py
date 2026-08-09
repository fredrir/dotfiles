import json
import re
from datetime import datetime

PREFERRED_INPUT_KEYS = (
    "command",
    "file_path",
    "path",
    "pattern",
    "query",
    "url",
    "description",
    "prompt",
)

RAW_FALLBACK_LIMIT = 400_000


def parse_time(value):
    if not value:
        return None
    try:
        stamp = datetime.fromisoformat(str(value))
    except ValueError:
        return None
    if stamp.tzinfo is not None:
        stamp = stamp.astimezone()
    return stamp


def tool_title(name, input_value):
    name = str(name or "tool")
    detail = ""
    if isinstance(input_value, dict):
        for key in PREFERRED_INPUT_KEYS:
            value = input_value.get(key)
            if value:
                detail = str(value)
                break
        else:
            if input_value:
                detail = json.dumps(input_value, ensure_ascii=False)
    elif input_value:
        detail = str(input_value)
    detail = " ".join(detail.split())
    if len(detail) > 60:
        detail = detail[:59] + "…"
    return f"{name} · {detail}" if detail else name


def tag_strippers(tags):
    return [
        re.compile(rf"<{tag}>.*?(?:</{tag}>|\Z)", re.DOTALL | re.IGNORECASE) for tag in tags
    ]


def strip_tags(text, patterns):
    for pattern in patterns:
        text = pattern.sub("", text)
    return text.strip()


def block_text(content, image_marker="[image]"):
    if content is None:
        return ""
    if isinstance(content, str):
        return content
    if isinstance(content, dict):
        for key in ("text", "content", "output", "stdout"):
            if key in content:
                return block_text(content[key], image_marker)
        return ""
    if isinstance(content, list):
        parts = []
        for item in content:
            if isinstance(item, str):
                parts.append(item)
            elif isinstance(item, dict):
                if "image" in str(item.get("type", "")):
                    parts.append(image_marker)
                elif "text" in item:
                    parts.append(str(item.get("text", "")))
                else:
                    nested = block_text(item.get("content") or item.get("output"), image_marker)
                    if nested:
                        parts.append(nested)
        return "\n".join(part for part in parts if part)
    return str(content)


def round_label(round_):
    for turn in round_.turns:
        if turn.kind == "me" and turn.body.strip():
            return turn.body.strip().splitlines()[0]
    return "Response"


def finish(session, rounds, raw, bad, total):
    session.rounds = [r for r in rounds if r.turns]
    for r in session.rounds:
        r.label = round_label(r)
    if not session.title:
        session.title = next(
            (r.label for r in session.rounds if any(t.kind == "me" for t in r.turns)), ""
        )
    if total and bad / total > 0.5:
        session.degraded = True
        session.raw_text = raw[:RAW_FALLBACK_LIMIT]
    return session
