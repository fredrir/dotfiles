"""Carry a finished sync over to the other machine.

`dotfile sync -p` is the two-machine round trip written out once: sync here,
push what is committed, then pull and sync there. Every step is checked, and
the first failure stops the run with the far side's own message, because a
half-applied sync between machines is worse than one that never started.
"""

import os
import shlex
import shutil
import subprocess
import sys

import typer

from tools.core import blocks
from tools.core.process import capture
from tools.dotfile import merge as merge_state
from tools.dotfile.report import plural
from tools.dotfile.state import log

HOSTS_FILE = "config/hosts.dotfile"

# The far side runs a non-interactive shell, which reads no .zshrc and so
# inherits no PATH from it. The commands install here on every machine.
REMOTE_PATH = 'export PATH="$HOME/.local/bin:$PATH"'


def stop(message, code=1):
    """`die` that can carry the far side's exit status back out of ssh."""
    print(f"dotfile: {message}", file=sys.stderr)
    raise typer.Exit(code)


def first_line(text):
    for line in text.splitlines():
        if line.strip():
            return line.strip()
    return ""


def known_hosts(ctx):
    # Imported here rather than at module scope: sysinfo's package pulls in the
    # whole collector, and every `dotfile` invocation would pay for it.
    from tools.utils.sysinfo import hosts as hosts_config

    path = os.path.join(ctx.root, HOSTS_FILE)
    if not os.path.isfile(path):
        stop(f"{HOSTS_FILE} is missing, so --push cannot tell which machines exist")
    try:
        return hosts_config.load_hosts(path), hosts_config
    except blocks.BlockError as error:
        stop(blocks.describe(error, HOSTS_FILE, "host"))


def choose_host(ctx, requested):
    """Resolve `--to`, or its absence, to the machine that receives the push."""
    if not shutil.which("ssh"):
        stop("ssh is not installed, so --push has no way to reach the other machine")
    hosts, hosts_config = known_hosts(ctx)
    local = hosts_config.resolve(hosts=hosts)
    if local not in hosts:
        local = ""
    if requested:
        if requested not in hosts:
            stop(f"unknown machine '{requested}' ({HOSTS_FILE} knows: {', '.join(hosts)})")
        if requested == local:
            stop(f"'{requested}' is this machine")
        return requested
    if not local:
        stop("cannot tell which machine this is; name the other one: dotfile sync -p --to <host>")
    others = [name for name in hosts if name != local]
    if not others:
        stop(f"{HOSTS_FILE} lists no machine besides '{local}'")
    if len(others) > 1:
        stop(f"'{local}' has several peers ({', '.join(others)}); name one with --to")
    return others[0]


def repo_directory(ctx):
    """Where the repository sits under $HOME, reused verbatim on the far side."""
    relative = os.path.relpath(ctx.root, ctx.home)
    if relative.startswith(".."):
        return "dotfiles"
    return relative


def remote_script(directory, *lines):
    # $HOME is expanded by the far side's shell; the path itself is quoted, so a
    # directory with a space in it survives the trip.
    return "\n".join([f'cd "$HOME"/{shlex.quote(directory)} || exit 1', *lines])


def git(ctx, *args):
    return capture(["git", "-C", ctx.root, *args])


def current_branch(ctx):
    result = git(ctx, "rev-parse", "--abbrev-ref", "HEAD")
    branch = result.stdout.strip()
    if result.returncode != 0 or not branch:
        stop(first_line(result.stderr) or "this repository has no branch checked out")
    if branch == "HEAD":
        stop("this machine is on a detached HEAD, so there is nothing to push")
    return branch


def push_branch(ctx, branch):
    if git(ctx, "status", "--porcelain").stdout.strip():
        log("note: this machine has uncommitted changes, they stay here")
    upstream = git(ctx, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}")
    if upstream.returncode != 0:
        stop(f"'{branch}' tracks no remote branch (git push -u origin {branch} once)")
    tracked = upstream.stdout.strip()
    counted = git(ctx, "rev-list", "--count", f"{tracked}..HEAD")
    if counted.returncode != 0:
        stop(first_line(counted.stderr) or f"cannot compare '{branch}' with {tracked}")
    ahead = int(counted.stdout.strip() or 0)
    if not ahead:
        log(f"nothing to push, {branch} matches {tracked}")
        return
    log(f"pushing {plural(ahead, 'commit')} to {tracked}")
    if subprocess.call(["git", "-C", ctx.root, "push"]) != 0:
        stop("git push failed")


def remote_state(host, directory):
    """The far side's branch and its uncommitted changes, in one round trip."""
    result = capture(["ssh", host, remote_script(directory, "git status --porcelain --branch")])
    if result.returncode != 0:
        stop(f"{host}: {first_line(result.stderr) or 'cannot read the repository there'}")
    branch = ""
    changes = []
    for line in result.stdout.splitlines():
        if line.startswith("## "):
            # `## main...origin/main [ahead 1]` -- the name is all that matters.
            branch = line[3:].split("...", 1)[0].split(" [", 1)[0]
            continue
        if line.strip():
            changes.append(line)
    return branch, changes


def discard_remote_tree(host, directory):
    log(f"discarding the working tree on {host}")
    result = capture(["ssh", host, remote_script(directory, "git reset --hard", "git clean -fd")])
    if result.returncode != 0:
        stop(f"{host}: {first_line(result.stderr) or 'cannot discard the working tree'}")
    for line in result.stdout.splitlines():
        log(f"  {line}")


def settle_remote_tree(host, directory, changes, force):
    log(f"{host} has uncommitted changes:")
    for line in changes:
        log(f"  {line}")
    if not force:
        if not (sys.stdin.isatty() and sys.stdout.isatty()):
            stop(f"{host}'s working tree is not clean, rerun with --force to discard it")
        if not typer.confirm(f"discard them on {host}?", default=True):
            stop("aborted")
    discard_remote_tree(host, directory)


def sync_remote(host, directory, resolve):
    command = "dotfile sync"
    if resolve != merge_state.SKIP:
        command += f" --resolve {resolve}"
    log(f"syncing {host}")
    # -t so the far side's prompts and progress render as they would in person.
    code = subprocess.call(
        ["ssh", "-t", host, remote_script(directory, REMOTE_PATH, "git pull || exit 1", command)]
    )
    if code != 0:
        stop(f"{host}: sync failed", code)


def cmd_push(ctx, host, force, resolve, dry_run):
    directory = repo_directory(ctx)
    branch = current_branch(ctx)
    log("")
    if dry_run:
        log(f"would push '{branch}', then pull and sync it on {host}:~/{directory}")
        return
    push_branch(ctx, branch)
    remote_branch, changes = remote_state(host, directory)
    if remote_branch != branch:
        stop(
            f"{host} is on '{remote_branch}' but this machine is on '{branch}'; "
            f"check '{branch}' out there first"
        )
    if changes:
        settle_remote_tree(host, directory, changes, force)
    sync_remote(host, directory, resolve)
