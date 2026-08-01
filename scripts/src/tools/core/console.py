import os
import sys

import typer
from rich.console import Console

stdout = Console(soft_wrap=True, highlight=False, markup=False)
stderr = Console(stderr=True, soft_wrap=True, highlight=False, markup=False)


def out(text=""):
    print(text)


def err(text=""):
    print(text, file=sys.stderr)


def die(prog, message, code=1):
    stderr.print(f"{prog}: {message}", style="red")
    raise typer.Exit(code)


def colors_enabled(stream=None):
    stream = stream if stream is not None else sys.stdout
    return stream.isatty() and "NO_COLOR" not in os.environ
