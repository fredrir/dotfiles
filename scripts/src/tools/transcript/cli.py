import sys
from datetime import datetime
from pathlib import Path
from typing import Annotated

import typer
from rich.table import Table

from tools.core import clipboard, menu
from tools.core.console import die, out, stdout
from tools.desktop.clean_copy import clean_text
from tools.transcript import config, detect, manage, redact, store, vault

app = typer.Typer(add_completion=False, help="Archive AI agent sessions as Obsidian notes.")

MENU = (
    ("capture", "wrap the clipboard into a transcript note"),
    ("import", "pick a recent session to import"),
    ("list", "list recent sessions"),
    ("add", "track a project for sync"),
    ("rm", "stop tracking a project"),
    ("sync", "sync allowlisted sessions now"),
)


@app.callback(invoke_without_command=True)
def main(ctx: typer.Context):
    if ctx.invoked_subcommand is not None:
        return
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        out(ctx.get_help())
        return
    choice = menu.pick("transcript", [name for name, _ in MENU], [text for _, text in MENU])
    if choice is None:
        return
    name = MENU[choice][0]
    if name == "capture":
        capture(provider="", raw=False, quiet=False, fallback="")
    elif name == "import":
        _interactive_import(15, False, False)
    elif name == "list":
        list_(limit=15)
    elif name == "add":
        _interactive_add()
    elif name == "rm":
        _interactive_rm()
    elif name == "sync":
        sync(dry_run=False, raw=False, quiet=False, tools=False)


def _untracked_candidates():
    allowed = config.allowed_projects()
    home = Path.home()
    cwds = [str(Path.cwd())]
    seen_cwds = set(cwds)
    for provider, path in store.all_sessions():
        cwd = store.peek_cwd(provider, path)
        if cwd and cwd not in seen_cwds:
            seen_cwds.add(cwd)
            cwds.append(cwd)
    candidates = []
    roots = set()
    for cwd in cwds:
        root = manage.resolve_repo(cwd)
        key = str(root).lower()
        if key in roots:
            continue
        roots.add(key)
        name = vault.project_of(str(root))
        if name.lower() in allowed or name in ("Home", "Unsorted"):
            continue
        try:
            hidden = any(part.startswith(".") for part in root.relative_to(home).parts)
        except ValueError:
            hidden = False
        if hidden:
            continue
        candidates.append((name, root))
    return candidates


def _interactive_add():
    page = 10
    candidates = _untracked_candidates()
    shown = page
    cursor = 0
    while True:
        visible = candidates[:shown]
        options = [name for name, _ in visible]
        descriptions = [str(root) for _, root in visible]
        remaining = len(candidates) - len(visible)
        if remaining:
            options.append(f"show {min(remaining, page)} more…")
            descriptions.append(f"{remaining} more from session history")
        options.append("enter a path…")
        descriptions.append("type a directory yourself")
        choice = menu.pick("track which project?", options, descriptions, default=cursor)
        if choice is None:
            return
        if remaining and choice == len(visible):
            menu.erase(len(options))
            cursor = len(visible)
            shown += page
            continue
        if choice == len(options) - 1:
            raw = typer.prompt("directory", default=".")
            directory = Path(raw).expanduser().resolve()
            if not directory.is_dir():
                die("transcript", f"no such directory: {directory}")
        else:
            directory = visible[choice][1]
        break
    default_name = manage.resolve_repo(directory).name
    name = typer.prompt("project name", default=default_name)
    group = typer.prompt("group (empty for none)", default="")
    project, added = manage.track(directory, name.strip(), group.strip())
    out(f"tracking {project}" if added else f"{project} is already tracked")


def _interactive_rm():
    projects = config.project_list()
    if not projects:
        die("transcript", "no tracked projects")
    choice = menu.pick("stop tracking which project?", projects)
    if choice is None:
        return
    name = projects[choice]
    if manage.untrack(name):
        out(f"stopped tracking {name}")
    else:
        die("transcript", f"{name} is not tracked")


def _redactor(raw):
    return redact.passthrough if raw else redact.redact


def _parse(provider, path):
    return store.parser_for(provider).parse(path)


def _recent(limit):
    rows = []
    for provider, path in store.all_sessions():
        session = _parse(provider, path)
        if not session.rounds and not session.degraded:
            continue
        rows.append((provider, path, session))
        if len(rows) >= limit:
            break
    return rows


def _print_table(rows):
    table = Table(header_style="bold")
    table.add_column("#", justify="right")
    table.add_column("provider")
    table.add_column("project")
    table.add_column("modified")
    table.add_column("rounds", justify="right")
    table.add_column("title")
    for index, (provider, path, session) in enumerate(rows, start=1):
        try:
            modified = datetime.fromtimestamp(path.stat().st_mtime).astimezone().strftime("%m-%d %H:%M")
        except OSError:
            modified = "?"
        table.add_row(
            str(index),
            provider,
            vault.project_of(session.cwd),
            modified,
            str(session.user_rounds),
            (session.title or "")[:60],
        )
    stdout.print(table)


def _save_import(session, raw, tools):
    if not session.rounds and not session.degraded:
        die("transcript", "session contains no conversation")
    note, updated = vault.save_session(session, "import", _redactor(raw), include_tools=tools)
    out(f"{'updated' if updated else 'created'} {note}")


def _interactive_import(limit, raw, tools):
    rows = _recent(limit)
    if not rows:
        die("transcript", "no sessions found")
    _print_table(rows)
    choice = typer.prompt("Import which session?", type=int, default=1)
    if choice < 1 or choice > len(rows):
        die("transcript", f"pick a number between 1 and {len(rows)}")
    _save_import(rows[choice - 1][2], raw, tools)


@app.command(help="Wrap clipboard text as a transcript note in the vault.")
def capture(
    provider: Annotated[
        str, typer.Option(help="Provider override; detected from content when empty.")
    ] = "",
    raw: Annotated[bool, typer.Option("--raw", help="Skip secret redaction.")] = False,
    quiet: Annotated[bool, typer.Option("--quiet", help="Print nothing on success.")] = False,
    fallback: Annotated[
        str, typer.Option("--fallback", help="Snapshot file used when the clipboard is empty.")
    ] = "",
):
    text = clipboard.read_text()
    if (text is None or not text.strip()) and fallback:
        snapshot = Path(fallback)
        try:
            text = clean_text(snapshot.read_text(errors="replace"))
        except OSError:
            text = None
        else:
            snapshot.unlink(missing_ok=True)
    if text is None or not text.strip():
        die("transcript", "clipboard is empty")
    name = provider or detect.provider_of(text)
    path = vault.save_capture(name, text, _redactor(raw))
    vault.add_daily_link(path, path.stem)
    if not quiet:
        out(str(path))


@app.command("import", help="Import a Claude Code or Codex session as a transcript note.")
def import_(
    target: Annotated[
        str, typer.Argument(help="Session jsonl path; omit to pick interactively.")
    ] = "",
    latest: Annotated[bool, typer.Option("--latest", help="Import the newest session.")] = False,
    limit: Annotated[int, typer.Option(help="Sessions listed in the picker.")] = 15,
    raw: Annotated[bool, typer.Option("--raw", help="Skip secret redaction.")] = False,
    tools: Annotated[bool, typer.Option("--tools", help="Include tool calls in the note.")] = False,
):
    if target:
        path = Path(target).expanduser()
        if not path.is_file():
            die("transcript", f"no such session file: {path}")
        session = _parse(store.provider_of_path(path), path)
    elif latest:
        sessions = store.all_sessions()
        if not sessions:
            die("transcript", "no sessions found")
        provider, path = sessions[0]
        session = _parse(provider, path)
    else:
        _interactive_import(limit, raw, tools)
        return
    _save_import(session, raw, tools)


@app.command("list", help="List recent Claude Code and Codex sessions.")
def list_(
    limit: Annotated[int, typer.Option(help="Number of sessions to show.")] = 15,
):
    rows = _recent(limit)
    if not rows:
        die("transcript", "no sessions found")
    _print_table(rows)


@app.command(help="Track a project for transcript sync (defaults to the current repo).")
def add(
    path: Annotated[str, typer.Argument(help="Project directory.")] = ".",
    name: Annotated[
        str, typer.Option(help="Project name; defaults to the repo directory name.")
    ] = "",
    group: Annotated[str, typer.Option(help="Group to file the project under.")] = "",
):
    directory = Path(path).expanduser().resolve()
    if not directory.is_dir():
        die("transcript", f"no such directory: {directory}")
    project, added = manage.track(directory, name, group)
    if added:
        out(f"tracking {project}")
    else:
        out(f"{project} is already tracked")


@app.command(help="Stop tracking a project; existing notes stay in the vault.")
def rm(
    target: Annotated[str, typer.Argument(help="Project name or directory.")],
):
    candidate = Path(target).expanduser()
    if candidate.exists():
        name = vault.project_of(str(manage.resolve_repo(candidate.resolve())))
    else:
        name = target.strip().strip("/")
    if manage.untrack(name):
        out(f"stopped tracking {name}")
    else:
        die("transcript", f"{name} is not tracked")


@app.command(help="Sync allowlisted Claude Code and Codex sessions into the vault.")
def sync(
    dry_run: Annotated[bool, typer.Option("--dry-run", help="Report without writing.")] = False,
    raw: Annotated[bool, typer.Option("--raw", help="Skip secret redaction.")] = False,
    quiet: Annotated[bool, typer.Option("--quiet", help="Print nothing on success.")] = False,
    tools: Annotated[bool, typer.Option("--tools", help="Include tool calls in the notes.")] = False,
):
    allowed = config.allowed_projects()
    threshold = config.min_rounds()
    index = vault.existing_notes()
    created = updated = 0
    for provider, path in store.all_sessions():
        cwd = store.peek_cwd(provider, path)
        if not cwd or vault.project_of(cwd).lower() not in allowed:
            continue
        note = index.get(store.guess_session_id(provider, path))
        try:
            source_mtime = path.stat().st_mtime
        except OSError:
            continue
        if note is not None and note.exists() and note.stat().st_mtime >= source_mtime:
            continue
        session = _parse(provider, path)
        if session.degraded and not session.rounds:
            continue
        if session.user_rounds < threshold:
            continue
        project = vault.project_of(session.cwd) if session.cwd else ""
        if project.lower() not in allowed:
            continue
        if dry_run:
            out(f"would sync {provider} {path.name} ({vault.project_of(session.cwd)})")
            continue
        _, existed = vault.save_session(
            session, "sync", _redactor(raw), index=index, include_tools=tools
        )
        if existed:
            updated += 1
        else:
            created += 1
    if not quiet and not dry_run:
        out(f"transcript sync: {created} created, {updated} updated")


if __name__ == "__main__":
    app()
