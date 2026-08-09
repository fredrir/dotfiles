import os
from pathlib import Path


def vault_root():
    return Path(os.environ.get("TRANSCRIPT_VAULT", os.path.expanduser("~/Documents/main")))


def transcripts_dir():
    return vault_root() / "Transcripts"


def claude_store():
    return Path(os.path.expanduser("~/.claude/projects"))


def codex_store():
    return Path(os.path.expanduser("~/.codex/sessions"))


def allowed_projects():
    raw = os.environ.get("TRANSCRIPT_PROJECTS", "dotfiles,ArchTeX")
    return {name.strip().lower() for name in raw.split(",") if name.strip()}


def min_rounds():
    try:
        return int(os.environ.get("TRANSCRIPT_MIN_ROUNDS", "2"))
    except ValueError:
        return 2
