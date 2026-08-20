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
from tools.dotfile import remove as remove_command
from tools.dotfile import system as system_cli
from tools.dotfile.secret import cli as secret_cli
from tools.dotfile.state import Context, die, log
from tools.theme import cli as theme_cli

app = typer.Typer(
    add_completion=False,
    help="Manage dotfile symlinks, packages, themes, and formatting for this repository.",
)

app.add_typer(secret_cli.app, name="secret")
app.add_typer(system_cli.app, name="system")
app.add_typer(theme_cli.app, name="theme")


@app.callback(invoke_without_command=True)
def main(ctx: typer.Context):
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


@app.command(help="Symlink every package in the profile manifest into $HOME.")
def link(
    profile: str | None = typer.Argument(None),
    dry_run: bool = typer.Option(
        False, "-n", "--dry-run", help="print actions without changing anything"
    ),
    override: list[str] = typer.Option(
        [], "--override", help="pick a machine override set: <group>=<name|none>"
    ),
):
    link_command.cmd_link(Context(), profile, dry_run, override)


@app.command(help="Materialise overlay-merged files, e.g. shared settings.json plus a platform overlay.")
def merge(
    profile: str | None = typer.Argument(None),
    dry_run: bool = typer.Option(
        False, "-n", "--dry-run", help="print actions without changing anything"
    ),
    force: bool = typer.Option(
        False, "--force", help="overwrite local edits with the merged result"
    ),
):
    merge_command.cmd_merge(Context(), profile, dry_run, force)


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


@app.command(help="Re-run setup non-interactively, skipping steps whose inputs are unchanged.")
def sync():
    ctx = Context()
    script = os.path.join(ctx.root, "setup.sh")
    if not os.access(script, os.X_OK):
        die("setup.sh is missing from the repository root")
    raise typer.Exit(subprocess.call([script, "--sync"]))


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
