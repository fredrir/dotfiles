import json
import re
from pathlib import Path

from tools.transcript import parseutil
from tools.transcript.model import Round, Session, Turn

SKIP_TYPES = {
    "mode",
    "permission-mode",
    "file-history-snapshot",
    "attachment",
    "system",
    "summary",
    "last-prompt",
    "progress",
    "queued-command",
    "todo",
    "plan",
}

WRAPPER_STRIPPERS = parseutil.tag_strippers(
    (
        "local-command-caveat",
        "local-command-stdout",
        "system-reminder",
        "command-message",
        "command-args",
        "command-name",
    )
)

COMMAND_NAME_RE = re.compile(r"<command-name>(.*?)</command-name>", re.DOTALL)
COMMAND_ARGS_RE = re.compile(r"<command-args>(.*?)</command-args>", re.DOTALL)


def clean_user_text(text):
    match = COMMAND_NAME_RE.search(text)
    if match:
        command = match.group(1).strip()
        args = COMMAND_ARGS_RE.search(text)
        if args and args.group(1).strip():
            command += " " + args.group(1).strip()
        return command
    return parseutil.strip_tags(text, WRAPPER_STRIPPERS)


def _attach_result(tool_turns, block):
    turn = tool_turns.get(block.get("tool_use_id"))
    if turn is None:
        return
    text = parseutil.block_text(block.get("content")).strip()
    if text:
        turn.body = f"{turn.body}\n{text}".strip() if turn.body else text


def _add_assistant(round_, content, tool_turns):
    if isinstance(content, str):
        content = [{"type": "text", "text": content}]
    if not isinstance(content, list):
        return
    for block in content:
        if not isinstance(block, dict):
            continue
        kind = block.get("type")
        if kind == "text":
            text = str(block.get("text", "")).strip()
            if not text:
                continue
            last = round_.turns[-1] if round_.turns else None
            if last is not None and last.kind == "turn":
                last.body += "\n\n" + text
            else:
                round_.turns.append(Turn("turn", "Response", text))
        elif kind == "tool_use":
            turn = Turn("tool", parseutil.tool_title(block.get("name"), block.get("input")), "")
            round_.turns.append(turn)
            call_id = block.get("id")
            if call_id:
                tool_turns[call_id] = turn


def parse(path):
    path = Path(path)
    session = Session(provider="claude", session_id=path.stem, source_path=str(path))
    try:
        raw = path.read_text(errors="replace")
    except OSError:
        session.degraded = True
        return session

    rounds = []
    current = None
    tool_turns = {}
    total = 0
    bad = 0

    for line in raw.splitlines():
        if not line.strip():
            continue
        total += 1
        try:
            entry = json.loads(line)
        except ValueError:
            bad += 1
            continue
        if not isinstance(entry, dict):
            bad += 1
            continue
        kind = entry.get("type")
        if kind == "ai-title":
            session.title = str(entry.get("aiTitle") or "").strip() or session.title
            continue
        if kind in SKIP_TYPES:
            continue
        if kind not in ("user", "assistant"):
            bad += 1
            continue
        if entry.get("isSidechain") or entry.get("isMeta"):
            continue
        session.cwd = session.cwd or entry.get("cwd") or ""
        session.session_id = entry.get("sessionId") or session.session_id
        stamp = parseutil.parse_time(entry.get("timestamp"))
        if session.started is None:
            session.started = stamp
        message = entry.get("message") or {}
        content = message.get("content")
        if kind == "assistant":
            session.model = message.get("model") or session.model
            if current is None:
                current = Round(timestamp=stamp)
                rounds.append(current)
            _add_assistant(current, content, tool_turns)
            continue
        blocks = content if isinstance(content, list) else [{"type": "text", "text": content}]
        texts = []
        for block in blocks:
            if not isinstance(block, dict):
                continue
            block_kind = block.get("type")
            if block_kind == "text":
                texts.append(str(block.get("text", "")))
            elif block_kind == "tool_result":
                _attach_result(tool_turns, block)
            elif block_kind == "image":
                texts.append("[image]")
        cleaned = clean_user_text("\n".join(texts)) if texts else ""
        if cleaned:
            current = Round(timestamp=stamp, turns=[Turn("me", "You", cleaned)])
            rounds.append(current)

    return parseutil.finish(session, rounds, raw, bad, total)
