from datetime import datetime
from pathlib import Path
from typing import Annotated

import typer
from rich.table import Table

from tools.core import clipboard
from tools.core.console import die, out, stdout
from tools.desktop.clean_copy import clean_text
from tools.transcript import config, detect, redact, store, vault

app = typer.Typer(add_completion=False, help="Archive AI agent sessions as Obsidian notes.")


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
        provider = store.provider_of_path(path)
        session = _parse(provider, path)
    elif latest:
        sessions = store.all_sessions()
        if not sessions:
            die("transcript", "no sessions found")
        provider, path = sessions[0]
        session = _parse(provider, path)
    else:
        rows = _recent(limit)
        if not rows:
            die("transcript", "no sessions found")
        _print_table(rows)
        choice = typer.prompt("Import which session?", type=int, default=1)
        if choice < 1 or choice > len(rows):
            die("transcript", f"pick a number between 1 and {len(rows)}")
        provider, path, session = rows[choice - 1]
    if not session.rounds and not session.degraded:
        die("transcript", "session contains no conversation")
    note, updated = vault.save_session(session, "import", _redactor(raw), include_tools=tools)
    out(f"{'updated' if updated else 'created'} {note}")


@app.command("list", help="List recent Claude Code and Codex sessions.")
def list_(
    limit: Annotated[int, typer.Option(help="Number of sessions to show.")] = 15,
):
    rows = _recent(limit)
    if not rows:
        die("transcript", "no sessions found")
    _print_table(rows)


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
        if not cwd or not ({part.lower() for part in Path(cwd).parts} & allowed):
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
        parts = {part.lower() for part in Path(session.cwd).parts} if session.cwd else set()
        if not parts & allowed:
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
