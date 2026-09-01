"""The `--completions <shell>` flag, and the callback behind every tool's tab.

The rust tools flatten `workstation::Completions` into their parser; the python
ones take `COMPLETIONS` as a parameter. Both then answer the same flag, which is
what lets `shared/zsh/conf.d/55-completions.zsh` treat the two halves of this
repository the same way.

The flag is eager, so it answers before a missing required argument does -- the
way `--help` already behaves -- and its callback reads the tree off the click
context, so no tool has to name itself twice.
"""

import importlib
import os

import typer

from tools.surface import introspect, values, zsh

FILENAME = "tools-completion.zsh"


def emit(ctx, value):
    if not value:
        return value
    root = ctx.find_root()
    program = root.info_name
    if not zsh.known_shell(value):
        available = ", ".join(zsh.SHELLS)
        typer.echo(f"{program}: no {value} completions; available: {available}", err=True)
        raise typer.Exit(2)
    typer.echo(zsh.script(introspect.from_click(root.command, program), program))
    raise typer.Exit(0)


COMPLETIONS = typer.Option(
    None,
    "--completions",
    metavar="SHELL",
    is_eager=True,
    callback=emit,
    help="Print shell completions and exit",
)


def register(app):
    """Add the hidden command a generated script calls back into for values."""

    @app.command("__complete", hidden=True)
    def complete(
        source: str = typer.Argument(...),
        arguments: list[str] | None = typer.Argument(None),
    ):
        for line in values.lines(source, list(arguments or ())):
            print(line)

    return complete


def programs():
    """Every installed command and the app behind it, read from pyproject."""
    import tomlkit

    from tools.core.paths import repo_root

    path = os.path.join(str(repo_root()), "scripts/python/pyproject.toml")
    with open(path, encoding="utf-8") as handle:
        data = tomlkit.parse(handle.read())
    found = dict(data["project"]["scripts"])
    backend = found.pop("dotfile-py", None)
    if backend:
        if backend.endswith(":run"):
            backend = backend.removesuffix(":run") + ":app"
        found["dotfile"] = backend
    return found


def load(target):
    module, _, attribute = target.partition(":")
    return getattr(importlib.import_module(module), attribute)


def trees():
    """The command tree of every installed command, skipping any that will not import."""
    found = {}
    for program, target in sorted(programs().items()):
        try:
            found[program] = introspect.from_typer(load(target), program)
        except Exception:  # a tool this machine cannot import still has no completions
            continue
    return found


def write_all(directory):
    """Every tool's script in one file, which is what the shell sources."""
    scripts = [zsh.script(tree, program) for program, tree in trees().items()]
    body = "\n".join(scripts)
    os.makedirs(directory, exist_ok=True)
    path = os.path.join(directory, FILENAME)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(body)
    return path, len(scripts)
