from typing import Annotated

import typer

from tools.core import blocks
from tools.core.console import die
from tools.utils.sysinfo import hosts
from tools.utils.sysinfo.bench.cli import app as bench_app
from tools.utils.sysinfo.bench.health import benchmark_issues
from tools.utils.sysinfo.collect import collect_snapshot
from tools.utils.sysinfo.health import health_issues
from tools.utils.sysinfo.models import RenderOptions
from tools.utils.sysinfo.plain import render_plain
from tools.utils.sysinfo.pretty import render_pretty
from tools.utils.sysinfo.view import build_view

PROG = "sysinfo"

app = typer.Typer(add_completion=False)
app.add_typer(bench_app, name="bench")


def current_host():
    # resolve() reads hosts.dotfile, so a stray brace in it used to surface as a
    # traceback from the plain hardware summary, which needs no hosts file at all.
    try:
        return hosts.resolve()
    except blocks.BlockError as error:
        die(PROG, hosts.describe_error(error))
        return ""


@app.callback(
    invoke_without_command=True,
    help="Summarise the environment and hardware of this machine.",
)
def sysinfo(
    ctx: typer.Context,
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
    if ctx.invoked_subcommand is not None:
        return
    options = RenderOptions(full=full, health=health)
    snapshot = collect_snapshot(full=full or pretty)
    view = build_view(snapshot)
    issues = health_issues(snapshot) + benchmark_issues(current_host())
    if pretty:
        render_pretty(view, issues, options)
    else:
        render_plain(view, issues, options)
