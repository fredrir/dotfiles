import os
import tarfile

import typer

from tools.core.console import colors_enabled, err, out
from tools.surface import entry as surface

app = typer.Typer(add_completion=False)

TAR_SUFFIXES = {
    ".tar.gz": "r:gz",
    ".tgz": "r:gz",
    ".tar.bz2": "r:bz2",
    ".tbz2": "r:bz2",
    ".tar.xz": "r:xz",
    ".txz": "r:xz",
    ".tar": "r:",
}

BOLD = "\033[1m"
DIM = "\033[2m"
CYAN = "\033[36m"
GREEN = "\033[32m"
RESET = "\033[0m"


def open_mode(archive):
    for suffix, mode in TAR_SUFFIXES.items():
        if archive.endswith(suffix):
            return mode
    return None


def directory_of(name, is_directory):
    entry = name.removeprefix("./")
    if not is_directory and not entry.endswith("/"):
        entry = entry.rpartition("/")[0]
    return entry.rstrip("/")


def collect(entries, max_depth):
    counts = {}
    directories = set()
    for name, is_directory in entries:
        directory = directory_of(name, is_directory)
        if not directory:
            continue
        parts = directory.split("/")
        if max_depth is not None and len(parts) > max_depth:
            continue
        current = ""
        for part in parts:
            current = part if not current else f"{current}/{part}"
            directories.add(current)
        counts[directory] = counts.get(directory, 0) + 1
    return counts, directories


def children_of(directories):
    children = {}
    for directory in sorted(directories):
        parent = directory.rpartition("/")[0]
        children.setdefault(parent, []).append(directory)
    return children


def render_tree(children, counts, styled):
    bold = BOLD if styled else ""
    dim = DIM if styled else ""
    cyan = CYAN if styled else ""
    green = GREEN if styled else ""
    reset = RESET if styled else ""

    lines = [
        f"{bold}{cyan}Archive directory tree{reset}",
        f"{dim}count = direct archive entries mapped to that directory{reset}",
        "",
    ]

    def walk(parent, prefix):
        entries = children.get(parent, [])
        for index, child in enumerate(entries):
            last = index == len(entries) - 1
            connector = "└─ " if last else "├─ "
            base = child.rpartition("/")[2]
            line = f"{prefix}{dim}{connector}{reset}{bold}{base}/{reset}"
            entry_count = counts.get(child, 0)
            if entry_count > 0:
                line += f"  {dim}[{green}{entry_count}{dim}]{reset}"
            lines.append(line)
            walk(child, prefix + ("   " if last else "│  "))

    walk("", "")
    return lines


@app.command(help="Show the directory tree of a tar archive with entry counts.")
def tardirs(
    archive: str = typer.Argument(...),
    max_depth: int | None = typer.Argument(None),
    completions: str = surface.COMPLETIONS,
):
    if not os.path.isfile(archive):
        err(f"File not found: {archive}")
        raise typer.Exit(1)
    mode = open_mode(archive)
    if mode is None:
        err(f"Unsupported archive: {archive}")
        raise typer.Exit(1)
    try:
        with tarfile.open(archive, mode) as handle:
            entries = [(member.name, member.isdir()) for member in handle.getmembers()]
    except (tarfile.TarError, OSError) as error:
        err(f"could not read {archive}: {error}")
        raise typer.Exit(1) from error
    counts, directories = collect(entries, max_depth)
    for line in render_tree(children_of(directories), counts, colors_enabled()):
        out(line)
