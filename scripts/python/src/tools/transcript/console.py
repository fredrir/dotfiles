import typer
from rich.console import Console

stdout = Console(soft_wrap=True, highlight=False, markup=False)
stderr = Console(stderr=True, soft_wrap=True, highlight=False, markup=False)


def out(text=""):
    print(text)


def die(prog, message, code=1):
    stderr.print(f"{prog}: {message}", style="red")
    raise typer.Exit(code)
