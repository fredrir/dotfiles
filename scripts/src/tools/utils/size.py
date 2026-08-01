import os
import sys

import typer

from tools.core.console import die, out
from tools.core.process import capture

app = typer.Typer(add_completion=False)


def run_du(arguments, label):
    result = capture(["du", *arguments])
    if result.stderr:
        sys.stderr.write(result.stderr)
    if result.returncode != 0 and not result.stdout:
        die("size", f"du failed for {label}")
    return result.stdout


def du_summary(target):
    output = run_du(["-sh", target], target)
    return output.split("\t", 1)[0]


def du_nonhidden_total(directory):
    entries = sorted(
        os.path.join(directory, name) for name in os.listdir(directory) if not name.startswith(".")
    )
    if not entries:
        return None
    output = run_du(["-sch", *entries], directory)
    total = None
    for line in output.splitlines():
        if line.endswith("total"):
            total = line.split("\t", 1)[0]
    return total


@app.command(help="Show total size of a file or directory.")
def size(
    target: str = typer.Argument(...),
    nonhidden: bool = typer.Option(
        False, "-d", help="only non-hidden items when given a directory"
    ),
):
    if not os.path.exists(target) and not os.path.islink(target):
        die("size", f"no such file or directory: {target}")
    if nonhidden and os.path.isdir(target):
        total = du_nonhidden_total(target)
        if total is not None:
            out(total)
    else:
        out(du_summary(target))
