from typing import Annotated

import typer

from tools.utils.sysinfo.collect import collect_snapshot
from tools.utils.sysinfo.health import health_issues
from tools.utils.sysinfo.models import RenderOptions
from tools.utils.sysinfo.plain import render_plain
from tools.utils.sysinfo.pretty import render_pretty
from tools.utils.sysinfo.view import build_view

app = typer.Typer(add_completion=False)


@app.command(help="Summarise the environment and hardware of this machine.")
def sysinfo(
    pretty: Annotated[
        bool,
        typer.Option("-p", "--pretty", help="show the complete branded hardware presentation"),
    ] = False,
    full: Annotated[
        bool,
        typer.Option("-f", "--full", help="include the extended inventory"),
    ] = False,
    health: Annotated[
        bool,
        typer.Option("-hh", "--health", help="explain active errors and warnings"),
    ] = False,
):
    options = RenderOptions(full=full, health=health)
    snapshot = collect_snapshot(full=full or pretty)
    view = build_view(snapshot)
    issues = health_issues(snapshot)
    if pretty:
        render_pretty(view, issues, options)
    else:
        render_plain(view, issues, options)
