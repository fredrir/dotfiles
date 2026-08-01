import typer

from tools.core.console import err
from tools.core.process import run

app = typer.Typer(add_completion=False)


@app.command(help="Stage everything, commit with the given message, and push.")
def gpp(message: list[str] = typer.Argument(...)):
    if run(["git", "add", "."]).returncode != 0:
        raise typer.Exit(1)
    staged = run(["git", "diff", "--cached", "--quiet"]).returncode
    if staged == 0:
        err("gpp: nothing to commit")
        raise typer.Exit(1)
    if staged != 1:
        raise typer.Exit(staged)
    if run(["git", "commit", "-m", " ".join(message)]).returncode != 0:
        raise typer.Exit(1)
    raise typer.Exit(run(["git", "push"]).returncode)
