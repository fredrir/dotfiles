import json
from pathlib import Path

from tools.transcript import claude, codex, config

PEEK_LINES = 60


def all_sessions():
    sessions = []
    claude_root = config.claude_store()
    if claude_root.is_dir():
        sessions.extend(("claude", path) for path in claude_root.rglob("*.jsonl"))
    codex_root = config.codex_store()
    if codex_root.is_dir():
        sessions.extend(("codex", path) for path in codex_root.rglob("*.jsonl"))

    def mtime(item):
        try:
            return item[1].stat().st_mtime
        except OSError:
            return 0.0

    return sorted(sessions, key=mtime, reverse=True)


def parser_for(provider):
    return claude if provider == "claude" else codex


def provider_of_path(path):
    path = Path(path).resolve()
    if str(path).startswith(str(config.codex_store().resolve())):
        return "codex"
    if str(path).startswith(str(config.claude_store().resolve())):
        return "claude"
    try:
        with open(path, errors="replace") as handle:
            first = handle.readline()
        entry = json.loads(first)
        if isinstance(entry, dict) and "payload" in entry:
            return "codex"
    except (OSError, ValueError):
        pass
    return "claude"


def guess_session_id(provider, path):
    if provider == "codex":
        return codex.session_id_from_name(path)
    return Path(path).stem


def peek_cwd(provider, path):
    try:
        with open(path, errors="replace") as handle:
            for _ in range(PEEK_LINES):
                line = handle.readline()
                if not line:
                    break
                try:
                    entry = json.loads(line)
                except ValueError:
                    continue
                if not isinstance(entry, dict):
                    continue
                if provider == "codex":
                    payload = entry.get("payload")
                    if isinstance(payload, dict) and payload.get("cwd"):
                        return payload["cwd"]
                elif entry.get("cwd"):
                    return entry["cwd"]
    except OSError:
        return ""
    return ""
