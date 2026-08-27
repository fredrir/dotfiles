import os
import subprocess

import typer

from tools.dotfile import add as add_command
from tools.dotfile import check as check_command
from tools.dotfile import format as format_command
from tools.dotfile import link as link_command
from tools.dotfile import merge as merge_command
from tools.dotfile import packages as packages_command
from tools.dotfile import profiles as profiles_command
from tools.dotfile import push as push_command
from tools.dotfile import remove as remove_command
from tools.dotfile import system as system_cli
from tools.dotfile.secret import cli as secret_cli
from tools.dotfile.state import Context, die, log
from tools.surface import entry as surface
from tools.theme import cli as theme_cli

app = typer.Typer(
    add_completion=False,
    help="Manage dotfile symlinks, packages, themes, and formatting for this repository.",
)

app.add_typer(secret_cli.app, name="secret")
app.add_typer(system_cli.app, name="system")
app.add_typer(theme_cli.app, name="theme")
surface.register(app)


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

RESOLVE_HELP = (
    "how to settle a config the live machine changed: "
    "skip leaves it and reports, repo discards the local change, live adopts it back"
)


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
        [], "--override", help="pick a machine override set: <group>=<name|none>"
    ),
    force: bool = typer.Option(False, "--force", help="alias for --resolve repo"),
    resolve: str = typer.Option(merge_command.SKIP, "--resolve", help=RESOLVE_HELP),
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


@app.command(
    hidden=True, help="Write every tool's completion script; setup.sh keeps it current."
)
def completions(
    directory: str = typer.Option(..., "--dir", help="directory to write the script into"),
):
    path, count = surface.write_all(os.path.expanduser(directory))
    log(f"  {count} tools completed by {path}")


@app.command(help="Regenerate the command tables in docs/cli from the tools themselves.")
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


@app.command(help="Regenerate config/packages.dotfile and PACKAGES.md.")
def packages():
    packages_command.cmd_packages(Context())


@app.command("format", help="Format tracked .conf files or the selected files.")
def format_conf(
    paths: list[str] | None = typer.Argument(None),
    stdin_name: str | None = typer.Option(
        None, "--stdin", help="format standard input as the named file"
    ),
):
    format_command.cmd_format(Context(), paths or [], stdin_name)


@app.command(help="Reconcile $HOME with the profile: link, merge, and apply secrets.")
def sync(
    profile: str | None = typer.Argument(None),
    dry_run: bool = typer.Option(
        False, "-n", "--dry-run", help="print actions without changing anything"
    ),
    override: list[str] = typer.Option(
        [], "--override", help="pick a machine override set: <group>=<name|none>"
    ),
    force: bool = typer.Option(
        False, "--force", help="alias for --resolve repo; with --push, discard without asking"
    ),
    resolve: str = typer.Option(merge_command.SKIP, "--resolve", help=RESOLVE_HELP),
    push: bool = typer.Option(
        False, "-p", "--push", help="then push, and pull and sync the other machine"
    ),
    to: str = typer.Option(
        "", "--to", help="which machine --push targets; the only other one by default"
    ),
):
    ctx = Context()
    script = os.path.join(ctx.root, "setup.sh")
    if not os.access(script, os.X_OK):
        die("setup.sh is missing from the repository root")
    checked_resolution(resolve)
    # Resolved before the local sync so an unreachable or misspelled target
    # fails now, rather than after the machine has already been relinked.
    host = push_command.choose_host(ctx, to) if push or to else ""
    command = [script, "--sync"]
    if profile:
        command.append(profile)
    # setup.sh owns the profile and override prompts, so everything the linker
    # alone cares about rides after `--`.
    passthrough = []
    for spec in override:
        passthrough += ["--override", spec]
    if dry_run:
        passthrough.append("-n")
    if force:
        passthrough.append("--force")
    if resolve != merge_command.SKIP:
        passthrough += ["--resolve", resolve]
    if passthrough:
        command += ["--", *passthrough]
    code = subprocess.call(command)
    if code or not host:
        raise typer.Exit(code)
    push_command.cmd_push(ctx, host, force, merge_command.REPO if force else resolve, dry_run)


@app.command(help="Show link state for every file in the profile.")
def status(profile: str | None = typer.Argument(None)):
    link_command.cmd_status(Context(), profile)


@app.command(help="Check the profile's links, required tools, and packages.")
def check(
    profile: str | None = typer.Argument(None),
    show_all: bool = typer.Option(
        False, "--all", help="list every finding instead of the first few"
    ),
):
    check_command.cmd_check(Context(), profile, show_all)


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
