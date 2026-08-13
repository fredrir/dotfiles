import typer

from tools.dotfile.secret import apply as apply_command
from tools.dotfile.secret import doctor as doctor_command
from tools.dotfile.secret import manage as manage_command
from tools.dotfile.secret import scan as scan_command
from tools.dotfile.secret import store as store_command
from tools.dotfile.state import Context, die, log

GROUP_FLAGS = (
    ("macos", "macos"),
    ("hyprland", "linux/hyprland"),
    ("kde", "linux/kde"),
    ("arch", "linux/arch"),
    ("ubuntu", "linux/ubuntu"),
    ("linux", "linux/common"),
    ("shared", "shared"),
)

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


@app.command(help="Encrypt a live file into the repository and keep it in place.")
def add(
    path: str = typer.Argument(...),
    pkg: str | None = typer.Option(None, "--pkg", help="package to place it in"),
    shared: bool = typer.Option(False, "--shared", help="place into shared (default)"),
    linux: bool = typer.Option(False, "--linux", help="place into linux/common"),
    arch: bool = typer.Option(False, "--arch", help="place into linux/arch"),
    ubuntu: bool = typer.Option(False, "--ubuntu", help="place into linux/ubuntu"),
    kde: bool = typer.Option(False, "--kde", help="place into linux/kde"),
    hyprland: bool = typer.Option(False, "--hyprland", help="place into linux/hyprland"),
    macos: bool = typer.Option(False, "--macos", help="place into macos"),
    marker: bool | None = typer.Option(
        None, "--marker/--no-marker", help="force the .secret package marker on or off"
    ),
):
    if pkg is not None and not pkg:
        die("--pkg needs a name")
    chosen = {
        "macos": macos,
        "hyprland": hyprland,
        "kde": kde,
        "arch": arch,
        "ubuntu": ubuntu,
        "linux": linux,
        "shared": shared,
    }
    group = "shared"
    for flag, target in GROUP_FLAGS:
        if chosen[flag]:
            group = target
            break
    store_command.cmd_add(Context(), path, group, pkg or "", marker)


@app.command(help="Open a tracked secret in $EDITOR and re-apply it.")
def edit(path: str = typer.Argument(...)):
    store_command.cmd_edit(Context(), path)


@app.command(help="Decrypt every tracked secret to its destination.")
def apply(
    dry_run: bool = typer.Option(
        False, "-n", "--dry-run", help="print actions without changing anything"
    ),
    force: bool = typer.Option(
        False, "--force", help="overwrite destinations edited on this machine"
    ),
):
    apply_command.cmd_apply(Context(), dry_run, force)


@app.command(help="Show what each tracked secret looks like on this machine.")
def status():
    apply_command.cmd_status(Context())


@app.command("vars", help="List the names templates can reference.")
def list_vars(
    unused: bool = typer.Option(False, "--unused", help="only names no template references"),
):
    store_command.cmd_vars(Context(), unused)


@app.command(help="Remove materialised secrets from their destinations.")
def clean(
    dry_run: bool = typer.Option(
        False, "-n", "--dry-run", help="print actions without changing anything"
    ),
):
    apply_command.cmd_clean(Context(), dry_run)
