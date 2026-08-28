import difflib
import os
import stat
import tempfile

import typer

from tools.core.console import colors_enabled
from tools.core.process import run
from tools.dotfile.report import DIM, GREEN, RED, YELLOW, paint
from tools.dotfile.secret.apply import prepare
from tools.dotfile.secret.variables import load as load_vars
from tools.dotfile.secret.vault import (
    ABSENT,
    CURRENT,
    DRIFTED,
    ENC,
    FAILED,
    PLAIN,
    SEALED,
    SYSTEM_MARKER,
    TMPL,
    UNRESOLVED,
    WROTE,
    package_entries,
)
from tools.dotfile.secret.vault import produce as vault_produce
from tools.dotfile.state import Context, die, each_package, log
from tools.dotfile.targets import load_targets

REFUSED = "refused"
UNREADABLE = "unreadable"

BLOCKING = (DRIFTED, FAILED, UNRESOLVED, REFUSED, UNREADABLE)

MARKS = {
    WROTE: ("wrote", GREEN),
    CURRENT: ("current", DIM),
    ABSENT: ("absent", YELLOW),
    SEALED: ("sealed", YELLOW),
    DRIFTED: ("drifted", RED),
    FAILED: ("failed", RED),
    UNRESOLVED: ("unresolved", RED),
    REFUSED: ("refused", RED),
    UNREADABLE: ("unreadable", RED),
}

FILE_MODE = 0o644
SECRET_MODE = 0o600
EXEC_MODE = 0o755

HINTS = (
    ("/etc/systemd/system/", "sudo systemctl daemon-reload"),
    (
        "/etc/systemd/network/",
        "sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=net",
    ),
    ("/etc/NetworkManager/", "sudo systemctl reload NetworkManager"),
    ("/etc/sysctl.d/", "sudo sysctl --system"),
)

app = typer.Typer(
    add_completion=False,
    help="Track root-owned files under /etc and install them as root.",
)


def plan(ctx):
    entries = []
    for state, pkgdir, name in each_package(ctx):
        if state == "system":
            entries.extend(package_entries(ctx, pkgdir, name, True))
    return sorted(entries, key=lambda entry: entry.dst)


def refusal(ctx, dst):
    if not dst.startswith("/"):
        return "destination is not absolute"
    if dst.rstrip("/") == "":
        return "destination is the filesystem root"
    if dst == ctx.home or dst.startswith(ctx.home + "/"):
        return "destination is under $HOME; use dotfile sync"
    top = "/" + dst.strip("/").split("/")[0]
    if not os.path.isdir(top):
        return f"{top} does not exist"
    return ""


def mode_for(entry):
    if entry.kind in (ENC, TMPL):
        return SECRET_MODE
    if os.access(entry.src, os.X_OK):
        return EXEC_MODE
    return FILE_MODE


def is_sealed(entry):
    try:
        metadata = os.stat(entry.dst)
    except OSError:
        return False
    return (
        entry.kind in (ENC, TMPL)
        and stat.S_IMODE(metadata.st_mode) == SECRET_MODE
        and metadata.st_uid == 0
        and metadata.st_gid == 0
    )


def needs_install(state):
    return state in (ABSENT, DRIFTED, SEALED)


def produce(ctx, entry, declared):
    if entry.kind == PLAIN:
        with open(entry.src, "rb") as handle:
            return handle.read(), ""
    return vault_produce(ctx, entry, declared)


def installed(entry):
    if not os.path.exists(entry.dst):
        return None, ABSENT
    try:
        with open(entry.dst, "rb") as handle:
            return handle.read(), ""
    except PermissionError:
        return (None, SEALED) if is_sealed(entry) else (None, UNREADABLE)
    except OSError:
        return None, UNREADABLE


def inspect(ctx, entry, declared):
    problem = refusal(ctx, entry.dst)
    if problem:
        entry.detail = problem
        return REFUSED
    wanted, problem = produce(ctx, entry, declared)
    if problem:
        return problem
    current, problem = installed(entry)
    if problem:
        return problem
    if current != wanted:
        return DRIFTED
    actual_mode = stat.S_IMODE(os.stat(entry.dst).st_mode)
    wanted_mode = mode_for(entry)
    if actual_mode != wanted_mode:
        entry.detail = f"mode {actual_mode:04o}, want {wanted_mode:04o}"
        return DRIFTED
    return CURRENT


def install_one(entry, data, dry):
    mode = mode_for(entry)
    descriptor, temp = tempfile.mkstemp()
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(data)
        command = [
            "sudo",
            "install",
            "-D",
            "-o",
            "root",
            "-g",
            "root",
            "-m",
            f"{mode:04o}",
            temp,
            entry.dst,
        ]
        if dry:
            log("  would: " + " ".join(command[:-2] + ["<rendered>", entry.dst]))
            return WROTE
        return WROTE if run(command).returncode == 0 else FAILED
    finally:
        os.remove(temp)


def show(state, entry, color_on):
    label, color = MARKS[state]
    detail = f"  {paint(entry.detail, DIM, color_on)}" if entry.detail else ""
    log(f"  {paint(label, color, color_on):<{len('unresolved') + 12}}{entry.dst}{detail}")


def counted(counts):
    return ", ".join(f"{count} {MARKS[state][0]}" for state, count in sorted(counts.items()))


def tally(results):
    counts = {}
    for state, _entry in results:
        counts[state] = counts.get(state, 0) + 1
    return counts


def survey(ctx):
    prepare(ctx)
    entries = plan(ctx)
    if not entries:
        return [], None
    declared = load_vars(ctx)
    if declared.note:
        log(f"  {declared.note}")
    return entries, declared


def hints_for(destinations):
    return [command for prefix, command in HINTS if any(d.startswith(prefix) for d in destinations)]


@app.callback(invoke_without_command=True)
def main(ctx: typer.Context):
    if ctx.invoked_subcommand is None:
        log(ctx.get_help())
        raise typer.Exit(0)


@app.command(help="Compare tracked system files with what is installed.")
def status():
    ctx = Context()
    entries, declared = survey(ctx)
    if not entries:
        log("no system files tracked")
        return
    color_on = colors_enabled()
    results = [(inspect(ctx, entry, declared), entry) for entry in entries]
    for state, entry in results:
        show(state, entry, color_on)
    counts = tally(results)
    log(counted(counts))
    if any(state in BLOCKING for state in counts):
        raise SystemExit(1)


@app.command(help="Show what would change on disk, without touching anything.")
def diff(path: str | None = typer.Argument(None)):
    ctx = Context()
    entries, declared = survey(ctx)
    if not entries:
        log("no system files tracked")
        return
    shown = 0
    for entry in entries:
        if path and path not in entry.dst and path not in entry.src:
            continue
        wanted, problem = produce(ctx, entry, declared)
        if problem:
            log(f"{entry.dst}: {MARKS[problem][0]} {entry.detail}".rstrip())
            continue
        current, problem = installed(entry)
        if problem == UNREADABLE:
            log(f"{entry.dst}: unreadable")
            continue
        if problem == SEALED:
            shown += 1
            log(f"{entry.dst}: sealed; private content is not readable without root")
            continue
        if current == wanted:
            continue
        shown += 1
        if entry.kind in (ENC, TMPL):
            log(f"{entry.dst}: private rendered content differs")
            continue
        before = (current or b"").decode("utf-8", errors="replace").splitlines(keepends=True)
        after = wanted.decode("utf-8", errors="replace").splitlines(keepends=True)
        log("".join(difflib.unified_diff(before, after, entry.dst, entry.src)).rstrip())
    if not shown:
        log("nothing to install")


@app.command(help="Install tracked system files to their destinations as root.")
def install(
    dry_run: bool = typer.Option(
        False, "-n", "--dry-run", help="print actions without changing anything"
    ),
    yes: bool = typer.Option(False, "--yes", help="do not ask before writing"),
):
    ctx = Context()
    entries, declared = survey(ctx)
    if not entries:
        log("no system files tracked")
        return
    color_on = colors_enabled()

    pending = []
    results = []
    for entry in entries:
        state = inspect(ctx, entry, declared)
        results.append((state, entry))
        if needs_install(state):
            pending.append(entry)

    blocked = [state for state in tally(results) if state in BLOCKING and state != DRIFTED]
    for state, entry in results:
        if state != CURRENT:
            show(state, entry, color_on)
    if blocked:
        log("")
        log("refusing to install while any file is unresolved")
        raise SystemExit(1)
    if not pending:
        log(f"nothing to install  {counted(tally(results))}")
        return

    log("")
    for entry in pending:
        log(f"  {mode_for(entry):04o} root:root  {entry.dst}")
    if not dry_run and not yes and not typer.confirm(f"install {len(pending)} file(s) as root?"):
        raise SystemExit(1)

    written = []
    for entry in pending:
        state = install_one(entry, produce(ctx, entry, declared)[0], dry_run)
        if state == FAILED:
            log(f"  failed {entry.dst}")
        else:
            written.append(entry.dst)

    log(f"{'would install' if dry_run else 'installed'} {len(written)} of {len(pending)}")
    for command in hints_for(written):
        log(f"  then: {command}")
    if len(written) != len(pending):
        raise SystemExit(1)


@app.command(help="Copy a root-owned file into the repository.")
def add(
    path: str = typer.Argument(...),
    pkg: str | None = typer.Option(None, "--pkg", help="package to place it in"),
    group: str = typer.Option("linux/arch", "--group", help="group directory to place it in"),
):
    ctx = Context()
    if not pkg:
        die("--pkg <name> is required")
    load_targets(ctx)
    src = os.path.abspath(os.path.expanduser(path))
    if not os.path.isfile(src):
        die(f"not a file: {src}")
    problem = refusal(ctx, src)
    if problem:
        die(problem)

    rel = src.lstrip("/")
    destrel = f"{group}/{pkg}/{rel}"
    dest = os.path.join(ctx.root, destrel)
    if os.path.exists(dest):
        die(f"already tracked: {destrel}")
    try:
        with open(src, "rb") as handle:
            data = handle.read()
    except OSError as error:
        die(f"cannot read {src}: {error}")

    os.makedirs(os.path.dirname(dest), exist_ok=True)
    with open(dest, "wb") as handle:
        handle.write(data)
    log(f"copied {src} -> {destrel}")

    marker = os.path.join(ctx.root, group, pkg, SYSTEM_MARKER)
    if not os.path.exists(marker):
        with open(marker, "w", encoding="utf-8"):
            pass
        log(f"marked {group}/{pkg} as a system package")

    top = rel.split("/")[0]
    mapline = f"{group}/{pkg}/{top} = /{top}"
    with open(ctx.targets_file, encoding="utf-8") as handle:
        present = mapline in handle.read().splitlines()
    if not present:
        with open(ctx.targets_file, "a", encoding="utf-8") as handle:
            handle.write(mapline + "\n")
        log(f"mapped {mapline}")
