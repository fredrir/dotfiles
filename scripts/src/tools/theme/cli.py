import typer

from tools.core.console import out
from tools.theme.model import Theme
from tools.theme.registry import EMITTERS
from tools.theme.render import Output

app = typer.Typer(add_completion=False)


@app.command(help="Stamp theme/palette.toml into every config that carries colors.")
def generate(
    list_outputs: bool = typer.Option(
        False,
        "--list-outputs",
        help="print the files this generator owns, one per line, and exit",
    ),
    stageable: bool = typer.Option(
        False,
        "--stageable",
        help="with --list-outputs, list only files that are safe to stage automatically",
    ),
    check: bool = typer.Option(
        False,
        "--check",
        help="report what would change without writing, and exit non-zero if anything would",
    ),
):
    if list_outputs:
        for emitter in EMITTERS:
            if stageable and not emitter.stageable:
                continue
            for target in emitter.outputs():
                out(target)
        return

    theme = Theme.load()
    output = Output(check=check)
    for emitter in EMITTERS:
        emitter.run(theme, output)

    if not output.changed:
        out("theme: already up to date")
        return

    out("theme: would regenerate" if check else "theme: regenerated")
    for target in output.changed:
        out(f"  {target}")
    if check:
        raise typer.Exit(1)
