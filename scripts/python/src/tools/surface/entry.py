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
    return dict(data["project"]["scripts"])


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
    from tools.surface import rust

    native = rust.tree("dotfile")
    if native is not None:
        found["dotfile"] = native
    return found
