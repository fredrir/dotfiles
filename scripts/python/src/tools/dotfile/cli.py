import os
import shutil

import typer

# typer vendors click, so this is where its exception types live; `typer.core`
# reaches for the same module to add its "did you mean" suggestions.
from typer._click.exceptions import UsageError
from typer.core import TyperGroup

from tools.dotfile import add as add_command
from tools.dotfile import doctor as doctor_command
from tools.dotfile import link as link_command
from tools.dotfile import merge as merge_command
from tools.dotfile import profiles as profiles_command
from tools.dotfile import remove as remove_command
from tools.dotfile import system as system_cli
from tools.dotfile.secret import cli as secret_cli
from tools.dotfile.state import Context, die, log
from tools.surface import entry as surface
from tools.theme import cli as theme_cli


class Dispatch(TyperGroup):
    """git's fallback rule: `dotfile <name>` runs `dotfile-<name>` off PATH."""

    def resolve_command(self, ctx, args):
        name = args[0] if args else ""
        external = bool(name) and not name.startswith("-") and self.get_command(ctx, name) is None
        if external and not ctx.resilient_parsing:
            program = f"dotfile-{name}"
            if shutil.which(program):
                os.execvp(program, [program, *args[1:]])
        try:
            return super().resolve_command(ctx, args)
        except UsageError as error:
            if name == "status":
                error.message = "'status' is included in 'dotfile doctor'; run that instead."
            elif name in {"docs", "packages"}:
                error.message = f"'{name}' is included in 'dotfile sync'; run that instead."
            elif external:
                error.message += " Run ./setup.sh"
            raise


app = typer.Typer(
    name="dotfile",
    cls=Dispatch,
    add_completion=False,
    help="The dotfile manager",
)

app.add_typer(secret_cli.app, name="secret")
app.add_typer(system_cli.app, name="system")
app.add_typer(theme_cli.app, name="theme")
surface.register(app)


def run():
    app(prog_name=os.environ.get("DOTFILE_PROGRAM_NAME"))


@app.callback(invoke_without_command=True)
def main(ctx: typer.Context, completions: str = surface.COMPLETIONS):
    if ctx.invoked_subcommand is None:
        log(ctx.get_help())
        raise typer.Exit(0)


GROUP_FLAGS = (
    ("macos", "macos"),
    ("server", "linux/server"),
    ("hyprland", "linux/hyprland"),
    ("kde", "linux/kde"),
    ("arch", "linux/arch"),
    ("ubuntu", "linux/ubuntu"),
    ("linux", "linux/common"),
    ("shared", "shared"),
)


RESOLUTIONS = (merge_command.SKIP, merge_command.REPO, merge_command.LIVE)


def checked_resolution(value):
    if value not in RESOLUTIONS:
        die(f"--resolve must be one of: {', '.join(RESOLUTIONS)}")
    return value


@app.command(
    hidden=True, help="Link and merge the profile. Use 'dotfile sync'; setup.sh calls this."
)
def link(
    profile: str | None = typer.Argument(None),
    dry_run: bool = typer.Option(
        False, "-n", "--dry-run", help="print actions without changing anything"
    ),
    override: list[str] = typer.Option(
        [], "--override", help="select a machine override with GROUP=NAME|none"
    ),
    force: bool = typer.Option(False, "--force", help="alias for --resolve repo"),
    resolve: str = typer.Option(merge_command.SKIP, "--resolve"),
):
    link_command.cmd_link(Context(), profile, dry_run, override, force, checked_resolution(resolve))


@app.command(help="Move a live config into the repo and symlink it back.")
def add(
    path: str = typer.Argument(...),
    shared: bool = typer.Option(False, "--shared", help="place into shared (default)"),
    linux: bool = typer.Option(False, "--linux", help="place into linux/common"),
    arch: bool = typer.Option(False, "--arch", help="place into linux/arch"),
    ubuntu: bool = typer.Option(False, "--ubuntu", help="place into linux/ubuntu"),
    kde: bool = typer.Option(False, "--kde", help="place into linux/kde"),
    hyprland: bool = typer.Option(False, "--hyprland", help="place into linux/hyprland"),
    server: bool = typer.Option(False, "--server", help="place into linux/server"),
    macos: bool = typer.Option(False, "--macos", help="place into macos"),
    pkg: str | None = typer.Option(None, "--pkg", help="override the package name"),
    description: str | None = typer.Option(
        None, "--description", "--desc", help="describe the package in PACKAGES.md"
    ),
):
    if pkg is not None and not pkg:
        die("--pkg needs a name")
    if description is not None and not description:
        die("--description needs text")
    chosen = {
        "macos": macos,
        "server": server,
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
    add_command.cmd_add(Context(), path, group, pkg or "", description or "")


@app.command(help="Move a tracked path out of the repo and keep it live.")
def remove(path: str = typer.Argument(...)):
    remove_command.cmd_remove(Context(), path)


@app.command(hidden=True, help="Write every tool's completion script; setup.sh keeps it current.")
def completions(
    directory: str = typer.Option(..., "--dir", help="directory to write the script into"),
):
    path, count = surface.write_all(os.path.expanduser(directory))
    log(f"  {count} tools completed by {path}")


@app.command("__reference", hidden=True)
def docs(
    check: bool = typer.Option(False, "--check", help="report drift instead of writing"),
):
    # Imported here because it reads every tool's parser, and the rust ones by
    # running them: work no other `dotfile` command should pay for.
    from tools.surface import docs as docs_module

    changed, missing = docs_module.write(surface.trees(), check)
    for program in sorted(set(missing)):
        log(f"  {program} is not built here, so its page was left alone")
    if not changed:
        log("  docs/cli is current")
        return
    for path in changed:
        log(f"  {'drifted' if check else 'updated'} {path}")
    if check:
        raise typer.Exit(1)


@app.command("__inventory", hidden=True)
def packages():
    from tools.dotfile import packages as packages_command

    packages_command.cmd_packages(Context())


@app.command(help="Refresh generated metadata and reconcile $HOME with the selected profile.")
def sync(
    profile: str | None = typer.Argument(None),
    dry_run: bool = typer.Option(
        False, "-n", "--dry-run", help="plan without changing files or contacting the peer"
    ),
    override: list[str] = typer.Option(
        [], "--override", help="pick a machine override set: <group>=<name|none>"
    ),
    force: bool = typer.Option(
        False,
        "--force",
        help="resolve local edits from the repository; discard remote edits with --push",
    ),
    resolve: str = typer.Option(merge_command.SKIP, "--resolve"),
    push: bool = typer.Option(
        False, "-p", "--push", help="push commits, then pull and sync the peer"
    ),
    to: str = typer.Option("", "--to", help="select the peer; implies --push"),
    verbose: bool = typer.Option(
        False,
        "-v",
        "--verbose",
        help="show every link, merge, generated file, and remote action",
    ),
):
    checked_resolution(resolve)
    die("sync is provided by the native dotfile executable")


@app.command(help="Check the profile's links, required tools, and packages.")
def doctor(
    profile: str | None = typer.Argument(None),
    show_all: bool = typer.Option(
        False, "--all", help="list every finding instead of the first few"
    ),
):
    doctor_command.cmd_doctor(Context(), profile, show_all)


@app.command(hidden=True)
def profiles(
    relevant: bool = typer.Option(False, "--relevant", help="only profiles matching this host"),
):
    ctx = Context()
    if relevant:
        names = profiles_command.list_relevant_profiles(ctx.environment_dir)
    else:
        names = profiles_command.list_profiles(ctx.environment_dir)
    for name in names:
        log(name)


@app.command("help", hidden=True)
def show_help(ctx: typer.Context):
    log(ctx.parent.get_help())
