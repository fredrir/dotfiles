import json
import sys
from typing import Annotated

import typer

from tools.core import blocks, menu
from tools.core.console import die, err, out
from tools.utils.sysinfo import hosts
from tools.utils.sysinfo.bench import document as document_module
from tools.utils.sysinfo.bench import report, runner, select, store
from tools.utils.sysinfo.bench.compare import compare_runs, regressions
from tools.utils.sysinfo.bench.health import benchmark_issues
from tools.utils.sysinfo.bench.limits import REGRESSION_PCT
from tools.utils.sysinfo.bench.record import TIERS
from tools.utils.sysinfo.bench.runner import FAMILIES, GateError

PROG = "sysinfo bench"

app = typer.Typer(add_completion=False, help="Measure this machine and compare runs over time.")

MENU = (
    ("run", "measure this machine now"),
    ("show", "inspect a stored run"),
    ("health", "warnings derived from benchmark history"),
    ("list", "stored runs"),
    ("compare", "two runs side by side"),
    ("trend", "one metric over time"),
    ("baseline", "set or clear the reference run"),
    ("prune", "thin old runs"),
)

COMPARISONS = (
    ("machine vs machine", "the same metric on two different machines"),
    ("before vs after upgrade", "two hardware configurations of one machine"),
    ("distro vs distro", "two installations on one machine"),
    ("pick two runs", "choose both sides by hand"),
)


def known_hosts():
    try:
        return hosts.load_hosts()
    except blocks.BlockError as error:
        die(PROG, hosts.describe_error(error))
        return {}


def adopt(known):
    detected = hosts.local_hostnames()
    primary = detected[0] if detected else "unknown"
    err(f"{PROG}: this machine is not described in hosts.dotfile (hostname {primary})")
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        die(PROG, "pass --host, or add a block to hosts.dotfile")
    choice = menu.pick(
        f"unknown machine: {primary}",
        ["adopt as a new host", "run without saving", "quit"],
        ["write a block into hosts.dotfile and pin it", "measure but persist nothing", ""],
    )
    if choice is None or choice == 2:
        raise typer.Exit(0)
    if choice == 1:
        return "", False
    name = typer.prompt("host name", default=primary.split(".", 1)[0])
    name = name.strip()
    if not name:
        die(PROG, "a host name is required")
    if name in known:
        die(PROG, f"{name} is already described in hosts.dotfile")
    role = typer.prompt("role", default="desktop")
    entry = hosts.Host(name=name, hostnames=detected, role=role.strip())
    path = hosts.append_host(entry)
    hosts.save_host(name)
    out(f"{PROG}: wrote {name} to {path} and pinned this machine to it")
    return name, True


def resolve_host(explicit="", allow_adopt=True):
    known = known_hosts()
    name = hosts.resolve(explicit, hosts=known)
    if name and name in known:
        return name, True
    if explicit:
        die(PROG, f"unknown host '{explicit}'; known hosts: {', '.join(known) or 'none'}")
    if not allow_adopt:
        die(PROG, "this machine is not described in hosts.dotfile")
    return adopt(known)


def pick_run(title, runs):
    if not runs:
        die(PROG, "no runs recorded")
    options = [run.run_id for run in runs]
    details = [report.describe_run(run) for run in runs]
    choice = menu.pick(title, options, details)
    if choice is None:
        raise typer.Exit(0)
    return runs[choice]


def pick_host(title):
    names = store.known_hosts()
    if not names:
        die(PROG, "no runs recorded")
    if len(names) == 1:
        return names[0]
    choice = menu.pick(title, names)
    if choice is None:
        raise typer.Exit(0)
    return names[choice]


def require_run(text, label):
    run = select.resolve(select.parse(text))
    if run is None:
        die(PROG, f"no run matches {label}")
    return run


@app.callback(invoke_without_command=True)
def main(ctx: typer.Context):
    if ctx.invoked_subcommand is not None:
        return
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        out(ctx.get_help())
        return
    choice = menu.pick("sysinfo bench", [name for name, _ in MENU], [text for _, text in MENU])
    if choice is None:
        return
    name = MENU[choice][0]
    if name == "run":
        run(tier="quick", only="", note="", tag=[], host="", workdir="", force=False,
            no_save=False, baseline=False, as_json=False)
    elif name == "show":
        show(target="")
    elif name == "health":
        health(host="")
    elif name == "list":
        list_(host="", limit=20, all_grades=False)
    elif name == "compare":
        interactive_compare()
    elif name == "trend":
        interactive_trend()
    elif name == "baseline":
        interactive_baseline()
    elif name == "prune":
        prune(host="", keep=12, dry_run=True)


def interactive_compare():
    choice = menu.pick("compare what?", [name for name, _ in COMPARISONS], [t for _, t in COMPARISONS])
    if choice is None:
        return
    if choice == 0:
        names = store.known_hosts()
        if len(names) < 2:
            die(PROG, "two machines are needed; only one has runs")
        first = menu.pick("first machine", names)
        if first is None:
            return
        rest = [name for name in names if name != names[first]]
        second = menu.pick("second machine", rest)
        if second is None:
            return
        left = require_run(names[first], names[first])
        right = require_run(rest[second], rest[second])
    elif choice == 1:
        host = pick_host("which machine?")
        found = select.epochs(host)
        if len(found) < 2:
            die(PROG, f"{host} has only one hardware configuration on record")
        keys = list(found)
        labels = [f"{epoch}  {len(found[epoch])} runs" for epoch in keys]
        first = menu.pick("earlier configuration", labels)
        if first is None:
            return
        second = menu.pick("later configuration", labels)
        if second is None:
            return
        left = found[keys[first]][0]
        right = found[keys[second]][0]
    elif choice == 2:
        host = pick_host("which machine?")
        found = select.installs(host)
        if len(found) < 2:
            die(PROG, f"{host} has runs from only one installation")
        keys = list(found)
        first = menu.pick("first installation", keys)
        if first is None:
            return
        second = menu.pick("second installation", keys)
        if second is None:
            return
        left = found[keys[first]][0]
        right = found[keys[second]][0]
    else:
        runs = store.list_runs(grades=select.ANY)
        left = pick_run("left run", runs)
        right = pick_run("right run", runs)
    emit_comparison(left, right)


def interactive_trend():
    host = pick_host("which machine?")
    runs = store.list_runs(host, grades=select.CLEAN)
    keys = sorted({metric.key for run in runs for metric in run.metrics})
    if not keys:
        die(PROG, f"{host} has no clean runs")
    choice = menu.pick("which metric?", keys)
    if choice is None:
        return
    report.render_trend(runs, keys[choice])


def interactive_baseline():
    host = pick_host("which machine?")
    runs = store.list_runs(host, grades=select.CLEAN)
    chosen = pick_run("use which run as the baseline?", runs)
    store.set_baseline(chosen.host, chosen.epoch, chosen.run_id)
    out(f"{PROG}: baseline for {chosen.host}@{chosen.epoch} is {chosen.run_id}")


def emit_comparison(left, right, as_json=False):
    deltas, changes, only_left, only_right = compare_runs(left, right)
    if as_json:
        out(json.dumps({
            "left": left.run_id,
            "right": right.run_id,
            "changes": [list(change) for change in changes],
            "deltas": [delta.__dict__ for delta in deltas],
        }, indent=2))
        return
    report.render_comparison(left, right, deltas, changes, only_left, only_right)


@app.command(help="Measure this machine and store the result.")
def run(
    tier: Annotated[str, typer.Option("--tier", help="quick, standard or heavy.")] = "quick",
    only: Annotated[str, typer.Option("--only", help="Comma separated families to measure.")] = "",
    note: Annotated[str, typer.Option("--note", help="Why this run was taken.")] = "",
    tag: Annotated[list[str] | None, typer.Option("--tag", help="Label for this run.")] = None,
    host: Annotated[str, typer.Option("--host", help="Record against this host.")] = "",
    workdir: Annotated[str, typer.Option("--workdir", help="Directory the disk tier writes in.")] = "",
    force: Annotated[bool, typer.Option("--force", help="Measure despite poor conditions.")] = False,
    no_save: Annotated[bool, typer.Option("--no-save", help="Print without storing.")] = False,
    baseline: Annotated[bool, typer.Option("--baseline", help="Pin this run as the baseline.")] = False,
    as_json: Annotated[bool, typer.Option("--json", help="Emit the run as JSON.")] = False,
):
    if tier not in TIERS:
        die(PROG, f"unknown tier '{tier}'; expected one of {', '.join(TIERS)}")
    families = tuple(part.strip() for part in only.split(",") if part.strip())
    for family in families:
        if family not in FAMILIES:
            die(PROG, f"unknown family '{family}'; expected one of {', '.join(FAMILIES)}")
    tags = tuple(tag or ())
    name, persist = resolve_host(host)
    persist = persist and not no_save
    def progress(kind, job, detail):
        if as_json:
            return
        if kind == "cool":
            err(f"  waiting for the machine to cool ({detail})")
        elif kind == "start":
            err(f"  {job} … ({detail})")
        elif kind == "done":
            err(f"  {job} done in {detail}")
        elif kind == "skip":
            err(f"  {job} skipped: {detail}")
    try:
        with store.exclusive():
            try:
                measured = runner.execute(
                    host=name or "unsaved",
                    tier=tier,
                    families=families,
                    note=note,
                    tags=tags,
                    force=force,
                    workdir=workdir,
                    report=progress,
                )
            except GateError as error:
                for reason in error.reasons:
                    err(f"{PROG}: {reason}")
                die(PROG, "conditions are not suitable; pass --force to measure anyway")
    except store.LockedError:
        die(PROG, "another benchmark is already running")
    if not measured.metrics:
        die(PROG, "no benchmark produced a result")
    if persist:
        store.save_run(measured)
    if as_json:
        out(json.dumps(measured.to_json(), indent=2))
    else:
        report.render_run(measured)
        if not persist:
            out("  not stored")
    if baseline and persist:
        store.set_baseline(measured.host, measured.epoch, measured.run_id)
        out(f"  baseline for {measured.host}@{measured.epoch} is now this run")
    reference = store.baseline_run(measured.host, measured.epoch)
    if reference and reference.run_id != measured.run_id:
        deltas, _changes, _left, _right = compare_runs(reference, measured)
        failed = regressions(deltas, REGRESSION_PCT)
        if failed and not as_json:
            out()
            for delta in failed:
                out(f"  regression: {delta.key} {delta.change_pct:+.1f}% against the baseline")
        if failed:
            raise typer.Exit(1)


@app.command(help="Show a stored run.")
def show(
    target: Annotated[str, typer.Argument(help="Selector such as archie or archie@a3f19c2e.")] = "",
    as_json: Annotated[bool, typer.Option("--json", help="Emit the run as JSON.")] = False,
):
    if target:
        found = require_run(target, target)
    else:
        found = pick_run("show which run?", store.list_runs(grades=select.ANY))
    if as_json:
        out(json.dumps(found.to_json(), indent=2))
        return
    report.render_run(found)


@app.command("list", help="List stored runs.")
def list_(
    host: Annotated[str, typer.Option("--host", help="Only this machine.")] = "",
    limit: Annotated[int, typer.Option("--limit", help="Rows to print.")] = 20,
    all_grades: Annotated[bool, typer.Option("--all", help="Include noisy and aborted runs.")] = False,
):
    grades = select.ANY if all_grades else select.CLEAN
    runs = store.list_runs(host or None, grades=grades)
    render = runs[:limit] if limit > 0 else runs
    report.render_list(render)
    if len(runs) > len(render):
        out(f"  {len(runs) - len(render)} more; pass --limit 0 for all")


@app.command(help="Report warnings derived from the benchmark history.")
def health(
    host: Annotated[str, typer.Option("--host", help="Machine to judge.")] = "",
):
    name = host or hosts.resolve()
    issues = benchmark_issues(name)
    if not issues:
        out(f"  no benchmark findings for {name or 'this machine'}")
        return
    out()
    for issue in issues:
        out(f"  {issue.severity}: {issue.title}")
        if issue.detail:
            out(f"    {issue.detail}")
        if issue.action:
            out(f"    {issue.action}")
    out()


@app.command(help="Compare two runs.")
def compare(
    left: Annotated[str, typer.Argument(help="Left selector.")] = "",
    right: Annotated[str, typer.Argument(help="Right selector.")] = "",
    as_json: Annotated[bool, typer.Option("--json", help="Emit the comparison as JSON.")] = False,
):
    if not left or not right:
        interactive_compare()
        return
    emit_comparison(require_run(left, left), require_run(right, right), as_json)


@app.command(help="Show one metric over time.")
def trend(
    target: Annotated[str, typer.Argument(help="Selector such as archie.")] = "",
    metric: Annotated[str, typer.Argument(help="Metric key such as cpu.multi.")] = "",
):
    if not target or not metric:
        interactive_trend()
        return
    selector = select.parse(target)
    runs = [run for run in store.list_runs(selector.host or None, grades=select.CLEAN)
            if select.matches(run, selector)]
    report.render_trend(runs, metric)


@app.command(help="Set or clear the baseline for a machine and hardware configuration.")
def baseline(
    action: Annotated[str, typer.Argument(help="set, clear or show.")] = "show",
    target: Annotated[str, typer.Argument(help="Selector such as archie@a3f19c2e.")] = "",
):
    if action == "show":
        pinned = store.load_baselines()
        if not pinned:
            out("  no baselines pinned")
            return
        for name in sorted(pinned):
            for epoch, run_id in sorted(pinned[name].items()):
                out(f"  {name}@{epoch}  {run_id}")
        return
    if action == "set":
        if not target:
            interactive_baseline()
            return
        chosen = require_run(target, target)
        store.set_baseline(chosen.host, chosen.epoch, chosen.run_id)
        out(f"{PROG}: baseline for {chosen.host}@{chosen.epoch} is {chosen.run_id}")
        return
    if action == "clear":
        selector = select.parse(target)
        if not selector.host or not selector.epoch:
            die(PROG, "clear needs a host and epoch, such as archie@a3f19c2e")
        if store.clear_baseline(selector.host, selector.epoch):
            out(f"{PROG}: cleared the baseline for {selector.host}@{selector.epoch}")
        else:
            out(f"{PROG}: no baseline was pinned for {selector.host}@{selector.epoch}")
        return
    die(PROG, f"unknown action '{action}'; expected set, clear or show")


@app.command(help="Regenerate benchmarks/BENCHMARKS.md from the stored runs.")
def document():
    path, changed = document_module.write()
    out(f"  {'updated' if changed else 'current'} {path}")


@app.command(help="Thin old runs, keeping baselines and the oldest run of each configuration.")
def prune(
    host: Annotated[str, typer.Option("--host", help="Only this machine.")] = "",
    keep: Annotated[int, typer.Option("--keep", help="Runs to keep per configuration.")] = 12,
    dry_run: Annotated[bool, typer.Option("--dry-run", help="Report without deleting.")] = False,
):
    dropped = store.prunable(host or None, keep=keep)
    if not dropped:
        out("  nothing to prune")
        return
    for run in dropped:
        out(f"  {'would remove' if dry_run else 'removed'} {run.host}/{run.run_id}")
    if dry_run:
        out(f"  {len(dropped)} runs would be removed; re-run without --dry-run")
        return
    for run in dropped:
        store.run_path(run.host, run.run_id).unlink(missing_ok=True)
    out(f"  removed {len(dropped)} runs")
