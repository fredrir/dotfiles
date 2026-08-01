import os

import typer

from tools.core.console import out
from tools.core.process import capture

app = typer.Typer(add_completion=False)


def existing_ancestor(resolved):
    probe = resolved
    if not os.path.isdir(probe):
        probe = os.path.dirname(probe)
    while not os.path.isdir(probe) and probe != "/":
        probe = os.path.dirname(probe)
    return probe


def git_toplevel(probe):
    result = capture(["git", "-C", probe, "rev-parse", "--show-toplevel"])
    if result.returncode != 0:
        return None
    return os.path.realpath(result.stdout.strip())


def describe(target):
    resolved = os.path.realpath(os.path.abspath(target))
    git_root = git_toplevel(existing_ancestor(resolved))
    if git_root is not None:
        if resolved == git_root:
            return "/"
        if resolved.startswith(git_root + "/"):
            return "/" + resolved[len(git_root) + 1 :]
    home = os.path.expanduser("~")
    if resolved == home:
        return "~"
    if resolved.startswith(home + "/"):
        return "~/" + resolved[len(home) + 1 :]
    return resolved


@app.command(help="Print the repository-relative or home-relative path of a target.")
def path(target: str = typer.Argument(".")):
    out(describe(target))
