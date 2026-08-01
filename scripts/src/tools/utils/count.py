import os

import typer

from tools.core.console import die, out

app = typer.Typer(add_completion=False)


def count_children(directory, nonhidden):
    total = 0
    with os.scandir(directory) as entries:
        for entry in entries:
            if nonhidden and entry.name.startswith("."):
                continue
            total += 1
    return total


def count_recursive(directory, nonhidden):
    total = 0
    for _parent, dirnames, filenames in os.walk(directory):
        if nonhidden:
            dirnames[:] = [name for name in dirnames if not name.startswith(".")]
            filenames = [name for name in filenames if not name.startswith(".")]
        total += len(dirnames) + len(filenames)
    return total


@app.command(help="Count items inside a directory.")
def count(
    directory: str = typer.Argument(...),
    recursive: bool = typer.Option(False, "-r", help="count recursively"),
    nonhidden: bool = typer.Option(False, "-d", help="only non-hidden items"),
):
    if not os.path.isdir(directory):
        die("count", f"not a directory: {directory}")
    if recursive:
        out(count_recursive(directory, nonhidden))
    else:
        out(count_children(directory, nonhidden))
