import typer

from tools.dotfile.secret import doctor as doctor_command
from tools.dotfile.secret import manage as manage_command
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
    no_canaries: bool = typer.Option(False, "--no-canaries", help="skip the private-value tier"),
    show_all: bool = typer.Option(
        False, "--all", help="list every finding instead of the first few"
    ),
):
    scan_command.cmd_scan(Context(), paths or [], staged, commits, not no_canaries, show_all)


@app.command(help="Create this machine's age identity and print its public key.")
def init():
    manage_command.cmd_init(Context())


@app.command(help="Add a recipient. With no key, enrols this machine.")
def enroll(
    label: str = typer.Argument(...),
    key: str | None = typer.Argument(None),
):
    manage_command.cmd_enroll(Context(), label, key or "")


@app.command(help="Remove a recipient and re-wrap what it could read.")
def revoke(label: str = typer.Argument(...)):
    manage_command.cmd_revoke(Context(), label)


@app.command(help="List the enrolled recipients.")
def keys():
    manage_command.cmd_keys(Context())


@app.command(help="Regenerate .sops.yaml from keys.dotfile.")
def sync(
    rewrap: bool = typer.Option(
        False, "--rewrap", help="also run sops updatekeys over every encrypted file"
    ),
):
    manage_command.cmd_sync(Context(), rewrap)


@app.command(help="Check identities, recipients, hooks, and encrypted files.")
def doctor(
    show_all: bool = typer.Option(False, "--all", help="also print the file locations"),
):
    doctor_command.cmd_doctor(Context(), show_all)
