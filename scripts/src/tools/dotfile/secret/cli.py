import typer

from tools.dotfile.secret import scan as scan_command
from tools.dotfile.state import Context, log

app = typer.Typer(
    add_completion=False,
    help="Keep private material out of the repository.",
)


@app.callback(invoke_without_command=True)
def main(ctx: typer.Context):
    if ctx.invoked_subcommand is None:
        log(ctx.get_help())
        raise typer.Exit(0)


@app.command(help="Scan for leaked tokens, private values, and encryption invariants.")
def scan(
    paths: list[str] | None = typer.Argument(None),
    staged: bool = typer.Option(False, "--staged", help="scan what is staged for commit"),
    commits: str | None = typer.Option(
        None, "--commits", help="scan blobs added in a rev-list range"
    ),
    no_canaries: bool = typer.Option(
        False, "--no-canaries", help="skip the private-value tier (use in CI)"
    ),
    show_all: bool = typer.Option(
        False, "--all", help="list every finding instead of the first few"
    ),
):
    scan_command.cmd_scan(Context(), paths or [], staged, commits, not no_canaries, show_all)
