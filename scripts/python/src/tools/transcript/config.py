import os
import tomllib
from functools import lru_cache
from pathlib import Path

DEFAULT_VAULT = "~/Documents/main"
DEFAULT_PROJECTS = ("dotfiles", "ArchTeX")


def _config_path():
    return Path(
        os.environ.get("TRANSCRIPT_CONFIG", os.path.expanduser("~/.config/transcript/config.toml"))
    )


@lru_cache(maxsize=8)
def _load_file(path_text):
    try:
        with open(path_text, "rb") as handle:
            return tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError):
        return {}


def _file_config():
    return _load_file(str(_config_path()))


def vault_root():
    value = os.environ.get("TRANSCRIPT_VAULT") or _file_config().get("vault") or DEFAULT_VAULT
    return Path(os.path.expanduser(str(value)))


def transcripts_dir():
    return vault_root() / "Transcripts"


def group_destinations():
    destinations = _file_config().get("destinations")
    if not isinstance(destinations, dict):
        return {}
    return {
        str(name).strip(): str(path).strip()
        for name, path in destinations.items()
        if str(name).strip() and str(path).strip()
    }


def destination_dir(group):
    value = next(
        (path for name, path in group_destinations().items() if name.lower() == group.lower()),
        None,
    )
    if value is None:
        return None
    relative = Path(value)
    if relative.is_absolute():
        raise ValueError(f"destination for {group} must be relative to the vault")
    root = vault_root()
    destination = root / relative
    if not destination.resolve().is_relative_to(root.resolve()):
        raise ValueError(f"destination for {group} must stay inside the vault")
    return destination


def claude_store():
    return Path(os.path.expanduser("~/.claude/projects"))


def codex_store():
    return Path(os.path.expanduser("~/.codex/sessions"))


def project_list():
    raw = os.environ.get("TRANSCRIPT_PROJECTS")
    if raw:
        names = raw.split(",")
    else:
        names = _file_config().get("projects") or list(DEFAULT_PROJECTS)
    return [str(name).strip() for name in names if str(name).strip()]


def allowed_projects():
    return {name.lower() for name in project_list()}


def min_rounds():
    raw = os.environ.get("TRANSCRIPT_MIN_ROUNDS") or _file_config().get("min_rounds")
    try:
        return int(raw)
    except (TypeError, ValueError):
        return 2


def project_aliases():
    aliases = _file_config().get("aliases")
    if not isinstance(aliases, dict):
        return {}
    result = {}
    for pattern, name in aliases.items():
        parts = tuple(part.lower() for part in str(pattern).split("/") if part)
        if parts and str(name).strip():
            result[parts] = str(name).strip()
    return result


def nested_groups():
    raw = _file_config().get("nested_groups")
    if not isinstance(raw, list):
        return set()
    return {str(name).strip().lower() for name in raw if str(name).strip()}


def project_groups():
    groups = _file_config().get("groups")
    if not isinstance(groups, dict):
        return {}
    return {
        str(name): {str(member).lower() for member in members}
        for name, members in groups.items()
        if isinstance(members, list)
    }
