import os
import subprocess

import typer

from tools.transcript.native import binary


def emit(ctx, value):
    if not value:
        return value
    program = ctx.find_root().info_name
    try:
        result = subprocess.run(
            [binary("dotfile"), "completions", "--program", program, "--shell", value],
            check=False,
            timeout=5,
        )
    except (OSError, RuntimeError, ValueError, subprocess.TimeoutExpired) as error:
        typer.echo(f"{program}: {error}", err=True)
        raise typer.Exit(1) from error
    raise typer.Exit(result.returncode)


COMPLETIONS = typer.Option(
    None, "--completions", metavar="SHELL", is_eager=True,
    callback=emit, help="Print shell completions and exit",
)


def values(source, arguments):
    from tools.transcript import config, detect, store

    if source == "projects":
        return config.project_list()
    if source == "groups":
        return sorted(config.group_destinations())
    if source == "providers":
        return sorted(detect.PROVIDER_MARKERS)
    if source == "sessions":
        limit = min(max(int(arguments[0]) if arguments else 25, 0), 1000)
        return [
            (str(path), f"{provider} {os.path.basename(path)}")
            for provider, path in store.all_sessions()[:limit]
        ]
    return []


def lines(source, arguments):
    def escape(value):
        return " ".join(str(value).split()).replace(":", r"\:")

    try:
        return [
            f"{escape(value[0])}:{' '.join(str(value[1]).split())}"
            if isinstance(value, tuple) else escape(value)
            for value in values(source, arguments) if value
        ]
    except (OSError, ValueError, KeyError, RuntimeError):
        return []


def register(app):
    @app.command("__complete", hidden=True)
    def complete(
        source: str = typer.Argument(...),
        arguments: list[str] | None = typer.Argument(None),
    ):
        for line in lines(source, arguments or []):
            print(line)

    return complete
