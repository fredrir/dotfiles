from dataclasses import dataclass
from pathlib import Path

from tools.transcript import config, vault


@dataclass(frozen=True)
class Move:
    group: str
    source: Path
    destination: Path


def _stored_group_name(configured_name):
    for group in config.project_groups():
        if group.lower() == configured_name.lower():
            return group
    for project in (*config.project_list(), vault.CAPTURES_PROJECT):
        folder = Path(vault.folder_for(project))
        if folder.parts[0].lower() == configured_name.lower():
            return folder.parts[0]
    return configured_name


def plan():
    moves = []
    for configured_name in config.group_destinations():
        group = _stored_group_name(configured_name)
        source_root = config.transcripts_dir() / group
        destination_root = config.destination_dir(configured_name)
        if destination_root is None:
            continue
        source_resolved = source_root.resolve()
        destination_resolved = destination_root.resolve()
        if source_resolved == destination_resolved:
            continue
        if destination_resolved.is_relative_to(source_resolved):
            raise ValueError(f"destination for {group} cannot be inside its current directory")
        if not source_root.is_dir():
            continue
        for source in sorted(source_root.rglob("*")):
            if source.is_dir():
                continue
            relative = source.relative_to(source_root)
            moves.append(Move(group, source, destination_root / relative))
    return moves


def conflicts(moves):
    found = []
    destinations = set()
    for move in moves:
        if move.destination.exists() or move.destination in destinations:
            found.append(move)
        destinations.add(move.destination)
    return found


def _remove_empty_directories(root):
    if not root.is_dir():
        return
    directories = sorted(
        (path for path in root.rglob("*") if path.is_dir()),
        key=lambda path: len(path.parts),
        reverse=True,
    )
    for directory in (*directories, root):
        try:
            directory.rmdir()
        except OSError:
            pass


def apply(moves):
    blocked = conflicts(moves)
    if blocked:
        raise FileExistsError(blocked[0].destination)
    source_roots = set()
    for move in moves:
        if move.destination.exists():
            raise FileExistsError(move.destination)
        move.destination.parent.mkdir(parents=True, exist_ok=True)
        move.source.rename(move.destination)
        source_roots.add(config.transcripts_dir() / move.group)
    for root in source_roots:
        _remove_empty_directories(root)
    return len(moves)
