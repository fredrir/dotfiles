import json
import re
from pathlib import Path

from tools.transcript import parseutil
from tools.transcript.model import Round, Session, Turn

SKIP_TYPES = {
    "world_state",
    "compacted",
    "turn_context",
    "event_msg",
    "session_meta",
    "response_item",
}

SKIP_PAYLOADS = {
    "reasoning",
    "web_search_call",
    "other",
}

WRAPPER_STRIPPERS = parseutil.tag_strippers(
    (
        "user_instructions",
        "environment_context",
        "turn_context",
        "collaboration_mode_context",
        "system_status",
        "app_context",
        "permissions_context",
    )
)

SESSION_ID_RE = re.compile(r"([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})")


def session_id_from_name(path):
    match = SESSION_ID_RE.search(Path(path).name)
    return match.group(1) if match else Path(path).stem


def _output_text(value):
    if isinstance(value, str):
        stripped = value.strip()
        if stripped.startswith("{"):
            try:
                return parseutil.block_text(json.loads(stripped))
            except ValueError:
                return value
        return value
    return parseutil.block_text(value)


def _tool_call(payload):
    kind = payload.get("type")
    if kind == "local_shell_call":
        action = payload.get("action") or {}
        command = action.get("command")
        if isinstance(command, list):
            command = " ".join(str(part) for part in command)
        return "shell", command
    name = payload.get("name")
    arguments = payload.get("arguments") or payload.get("input")
    if isinstance(arguments, str):
        stripped = arguments.strip()
        if stripped.startswith("{"):
            try:
                arguments = json.loads(stripped)
            except ValueError:
                pass
    return name, arguments


def parse(path):
    path = Path(path)
    session = Session(
        provider="codex", session_id=session_id_from_name(path), source_path=str(path)
    )
    try:
        raw = path.read_text(errors="replace")
    except OSError:
        session.degraded = True
        return session

    rounds = []
    current = None
    tool_turns = {}
    fallback_events = []
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
        payload = entry.get("payload")
        payload = payload if isinstance(payload, dict) else {}
        stamp = parseutil.parse_time(entry.get("timestamp") or payload.get("timestamp"))

        if kind == "session_meta":
            session.session_id = str(
                payload.get("id") or payload.get("session_id") or session.session_id
            )
            session.cwd = payload.get("cwd") or session.cwd
            session.started = parseutil.parse_time(payload.get("timestamp")) or session.started
            continue
        if kind == "turn_context":
            session.model = payload.get("model") or session.model
            session.cwd = session.cwd or payload.get("cwd") or ""
            continue
        if kind == "event_msg":
            event_kind = payload.get("type")
            if event_kind == "user_message":
                fallback_events.append(("me", parseutil.block_text(payload.get("message")), stamp))
            elif event_kind == "agent_message":
                fallback_events.append(
                    ("turn", parseutil.block_text(payload.get("message")), stamp)
                )
            continue
        if kind != "response_item":
            if kind not in SKIP_TYPES:
                bad += 1
            continue

        item_kind = payload.get("type")
        if item_kind in SKIP_PAYLOADS or item_kind is None:
            continue
        if item_kind == "message":
            role = payload.get("role")
            text = parseutil.block_text(payload.get("content")).strip()
            if role == "user":
                cleaned = parseutil.strip_tags(text, WRAPPER_STRIPPERS)
                if cleaned.startswith(("# AGENTS.md", "Caveat:")):
                    cleaned = ""
                if cleaned:
                    current = Round(timestamp=stamp, turns=[Turn("me", "You", cleaned)])
                    rounds.append(current)
            elif role == "assistant" and text:
                if current is None:
                    current = Round(timestamp=stamp)
                    rounds.append(current)
                last = current.turns[-1] if current.turns else None
                if last is not None and last.kind == "turn":
                    last.body += "\n\n" + text
                else:
                    current.turns.append(Turn("turn", "Response", text))
            continue
        if item_kind in ("function_call", "custom_tool_call", "local_shell_call"):
            name, arguments = _tool_call(payload)
            turn = Turn("tool", parseutil.tool_title(name, arguments), "")
            if current is None:
                current = Round(timestamp=stamp)
                rounds.append(current)
            current.turns.append(turn)
            call_id = payload.get("call_id") or payload.get("id")
            if call_id:
                tool_turns[call_id] = turn
            continue
        if item_kind in ("function_call_output", "custom_tool_call_output"):
            turn = tool_turns.get(payload.get("call_id"))
            if turn is not None:
                text = _output_text(payload.get("output")).strip()
                if text:
                    turn.body = f"{turn.body}\n{text}".strip() if turn.body else text
            continue

    if not any(any(t.kind == "me" for t in r.turns) for r in rounds) and fallback_events:
        rounds = []
        current = None
        for kind, text, stamp in fallback_events:
            text = text.strip()
            if not text:
                continue
            if kind == "me":
                cleaned = parseutil.strip_tags(text, WRAPPER_STRIPPERS)
                if not cleaned or cleaned.startswith(("# AGENTS.md", "Caveat:")):
                    continue
                current = Round(timestamp=stamp, turns=[Turn("me", "You", cleaned)])
                rounds.append(current)
            else:
                if current is None:
                    current = Round(timestamp=stamp)
                    rounds.append(current)
                current.turns.append(Turn("turn", "Response", text))

    return parseutil.finish(session, rounds, raw, bad, total)
