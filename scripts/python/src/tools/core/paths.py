import os
from pathlib import Path


def repo_root():
    start = Path(__file__).resolve()
    for parent in start.parents:
        if (parent / "environment").is_dir() and (parent / "config" / "targets.dotfile").is_file():
            return parent
        if (parent / ".git").exists():
            return parent
    raise SystemExit("tools: dotfiles repository root not found")


def dotfiles_root():
    override = os.environ.get("DOTFILE_ROOT")
    if override:
        return Path(override).resolve()
    return repo_root()


def home():
    return Path(os.path.expanduser("~"))


def tilde(path):
    text = str(path)
    prefix = str(home())
    if text == prefix or text.startswith(prefix + "/"):
        return "~" + text[len(prefix) :]
    return text
