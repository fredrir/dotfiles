import json
import os
import shutil
import subprocess
from pathlib import Path

import typer


def dotfile_binary():
    manifest = os.environ.get("DOTFILE_DEV_BUILD_MANIFEST")
    if manifest:
        selected = None
        for line in Path(manifest).read_text().splitlines():
            artifact = json.loads(line)
            if (
                artifact.get("reason") == "compiler-artifact"
                and not artifact.get("profile", {}).get("test", False)
                and artifact.get("target", {}).get("name") == "dotfile"
                and artifact.get("executable")
            ):
                selected = Path(artifact["executable"])
        if selected and selected.is_file() and os.access(selected, os.X_OK):
            return str(selected)
        raise RuntimeError("prepared native binary missing: dotfile")
    if found := shutil.which("dotfile"):
        return found
    raise RuntimeError("dotfile not found; run ./setup.sh")


def emit(ctx, value):
    if not value:
        return value
    program = ctx.find_root().info_name
    try:
        result = subprocess.run(
            [dotfile_binary(), "completions", "--program", program, "--shell", value],
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
