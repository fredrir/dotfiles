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

MACHINES = "machine vs machine"
UPGRADE = "before vs after upgrade"
DISTROS = "distro vs distro"
BY_HAND = "pick two runs"

COMPARISONS = (
    (MACHINES, "the same metric on two different machines"),
    (UPGRADE, "two hardware configurations of one machine"),
    (DISTROS, "two installations on one machine"),
    (BY_HAND, "choose both sides by hand"),
)

PAIR_TITLES = {
    ("epoch", "a"): "earlier configuration",
    ("epoch", "b"): "later configuration",
    ("install", "a"): "first installation",
    ("install", "b"): "second installation",
}


def known_hosts():
    try:
        return hosts.load_hosts()
    except blocks.BlockError as error:
        die(PROG, hosts.describe_error(error))
        return {}


def current_host(explicit=""):
    try:
        return explicit or hosts.resolve()
    except blocks.BlockError as error:
        die(PROG, hosts.describe_error(error))
        return ""


def adopt(known):
    detected = hosts.local_hostnames()
    primary = detected[0] if detected else "unknown"
    err(f"{PROG}: this machine is not described in config/hosts.dotfile (hostname {primary})")
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        die(PROG, "pass --host, or add a block to config/hosts.dotfile")
    choice = menu.pick(
        f"unknown machine: {primary}",
        ["adopt as a new host", "run without saving", "quit"],
        ["write a block into config/hosts.dotfile and pin it", "measure but persist nothing", ""],
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
        die(PROG, f"{name} is already described in config/hosts.dotfile")
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
        die(PROG, "this machine is not described in config/hosts.dotfile")
    return adopt(known)


def require_terminal(what):
    # menu.cascade returns None when stdout is not a terminal, which every caller
    # read as "the user quit" -- so piping these commands printed nothing and
    # exited 0, as though the work had been done.
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        die(PROG, f"{what} needs a terminal; pass the arguments instead")


class _Runs:
    def __init__(self):
        self.cache = {}
        self.seen = {}

    def hosts(self):
        if "hosts" not in self.cache:
            self.cache["hosts"] = store.known_hosts()
        return self.cache["hosts"]

    def listed(self, host=None, grades=select.ANY):
        key = (host, grades)
        if key not in self.cache:
            found = store.list_runs(host, grades=grades)
            self.cache[key] = found
            self.seen.update({one.run_id: one for one in found})
        return self.cache[key]

    def groups(self, host, kind):
        key = (kind, host)
        if key not in self.cache:
            self.cache[key] = select.epochs(host) if kind == "epoch" else select.installs(host)
        return self.cache[key]

    def run(self, run_id):
        return self.seen[run_id]


def _option(picks, kind):
    found = next((pick for pick in picks if pick.kind == kind), None)
    return found.option if found else ""


def _host_of(picks, names):
    return _option(picks, "host") or (names[0] if names else "")


def _grouping(picks):
    if _option(picks, "compare") == UPGRADE:
        return "epoch", "hardware configuration"
    return "install", "installation"


def _note(message):
    return menu.Column([message], kind="note")


def _expand(runs, flow=""):
    def host_column(title):
        names = runs.hosts()
        if len(names) < 2:
            return None
        return menu.Column(names, kind="host", title=title)

    def run_column(found, title, kind="run"):
        if not found:
            return _note("no runs recorded")
        details = [report.describe_run(one) for one in found]
        return menu.Column([one.run_id for one in found], details, kind=kind, title=title)

    def pair(picks, side):
        host = _host_of(picks, runs.hosts())
        kind, noun = _grouping(picks)
        found = runs.groups(host, kind)
        if len(found) < 2:
            return _note(f"{host} has only one {noun} on record")
        keys = [key for key in found if side == "a" or key != _option(picks, f"{kind}-a")]
        details = [f"{len(found[key])} runs" for key in keys]
        return menu.Column(keys, details, kind=f"{kind}-{side}", title=PAIR_TITLES[kind, side])

    def after_host(name, picks):
        host = _host_of(picks, runs.hosts())
        if name == "trend":
            clean = runs.listed(host, select.CLEAN)
            keys = sorted({metric.key for one in clean for metric in one.metrics})
            if not keys:
                return _note(f"{host} has no clean runs")
            return menu.Column(keys, kind="metric", title="which metric?")
        if name == "baseline":
            title = "use which run as the baseline?"
            return run_column(runs.listed(host, select.CLEAN), title)
        return pair(picks, "a")

    def opening(name, picks):
        if name == "show":
            return run_column(runs.listed(), "show which run?")
        if name == "compare":
            options = [name for name, _ in COMPARISONS]
            details = [text for _, text in COMPARISONS]
            return menu.Column(options, details, kind="compare", title="compare what?")
        if name in ("trend", "baseline"):
            return host_column("which machine?") or after_host(name, picks)
        return None

    def expand(picks):
        if not picks:
            if flow:
                return opening(flow, picks)
            options = [name for name, _ in MENU]
            return menu.Column(options, [text for _, text in MENU], kind="menu")
        last = picks[-1]
        if last.kind == "menu":
            return opening(last.option, picks)
        if last.kind == "compare":
            if last.option == MACHINES:
                names = runs.hosts()
                if len(names) < 2:
                    return _note("two machines are needed; only one has runs")
                return menu.Column(names, kind="host-a", title="first machine")
            if last.option == BY_HAND:
                return run_column(runs.listed(), "left run", kind="run-a")
            return host_column("which machine?") or pair(picks, "a")
        if last.kind == "host":
            return after_host(flow or picks[0].option, picks)
        if last.kind == "host-a":
            rest = [name for name in runs.hosts() if name != last.option]
            return menu.Column(rest, kind="host-b", title="second machine")
        if last.kind == "run-a":
            return run_column(runs.listed(), "right run", kind="run-b")
        if last.kind in ("epoch-a", "install-a"):
            return pair(picks, "b")
        return None

    return expand


def _sides(runs, picks):
    comparison = _option(picks, "compare")
    if comparison == MACHINES:
        left, right = _option(picks, "host-a"), _option(picks, "host-b")
        return require_run(left, left), require_run(right, right)
    if comparison == BY_HAND:
        return runs.run(_option(picks, "run-a")), runs.run(_option(picks, "run-b"))
    kind, _noun = _grouping(picks)
    found = runs.groups(_host_of(picks, runs.hosts()), kind)
    return found[_option(picks, f"{kind}-a")][0], found[_option(picks, f"{kind}-b")][0]


def _open(flow, what):
    require_terminal(what)
    runs = _Runs()
    _dispatch(runs, menu.cascade(PROG, _expand(runs, flow=flow)), flow=flow)


def _pick_run(what):
    require_terminal(what)
    runs = _Runs()
    picks = menu.cascade(PROG, _expand(runs, flow="show"))
    if picks is None:
        return None
    if picks[-1].kind == "note":
        die(PROG, picks[-1].option)
    return runs.run(_option(picks, "run"))


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
    runs = _Runs()
    _dispatch(runs, menu.cascade(PROG, _expand(runs)))


def _dispatch(runs, picks, flow=""):
    if picks is None:
        return
    if picks[-1].kind == "note":
        die(PROG, picks[-1].option)
    name = flow or picks[0].option
    if name == "run":
        run(
            tier="quick",
            only="",
            note="",
            tag=[],
            host="",
            workdir="",
            force=False,
            no_save=False,
            baseline=False,
            as_json=False,
        )
    elif name == "show":
        report.render_run(runs.run(_option(picks, "run")))
    elif name == "health":
        health(host="")
    elif name == "list":
        list_(host="", limit=20, all_grades=False)
    elif name == "compare":
        emit_comparison(*_sides(runs, picks))
    elif name == "trend":
        host = _host_of(picks, runs.hosts())
        report.render_trend(runs.listed(host, select.CLEAN), _option(picks, "metric"))
    elif name == "baseline":
        chosen = runs.run(_option(picks, "run"))
        store.set_baseline(chosen.host, chosen.epoch, chosen.run_id)
        out(f"{PROG}: baseline for {chosen.host}@{chosen.epoch} is {chosen.run_id}")
    elif name == "prune":
        # Not forced to --dry-run any more: the menu entry says "thin old runs",
        # and the confirmation below is what makes actually doing so safe.
        prune(host="", keep=12, dry_run=False, yes=False)


def emit_comparison(left, right, as_json=False):
    deltas, changes, only_left, only_right = compare_runs(left, right)
    if as_json:
        out(
            json.dumps(
                {
                    "left": left.run_id,
                    "right": right.run_id,
                    "changes": [list(change) for change in changes],
                    "deltas": [delta.__dict__ for delta in deltas],
                },
                indent=2,
            )
        )
        return
    report.render_comparison(left, right, deltas, changes, only_left, only_right)


@app.command(help="Measure this machine and store the result.")
def run(
    tier: Annotated[str, typer.Option("--tier", help="quick, standard or heavy.")] = "quick",
    only: Annotated[str, typer.Option("--only", help="Comma separated families to measure.")] = "",
    note: Annotated[str, typer.Option("--note", help="Why this run was taken.")] = "",
    tag: Annotated[list[str] | None, typer.Option("--tag", help="Label for this run.")] = None,
    host: Annotated[str, typer.Option("--host", help="Record against this host.")] = "",
    workdir: Annotated[
        str, typer.Option("--workdir", help="Directory the disk tier writes in.")
    ] = "",
    force: Annotated[
        bool, typer.Option("--force", help="Measure despite poor conditions.")
    ] = False,
    no_save: Annotated[bool, typer.Option("--no-save", help="Print without storing.")] = False,
    baseline: Annotated[
        bool, typer.Option("--baseline", help="Pin this run as the baseline.")
    ] = False,
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

    # Everything that writes to the store stays inside the lock. Storing the run
    # and pinning the baseline used to happen after it was released, so a second
    # benchmark could interleave with either.
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
            if not measured.metrics:
                die(PROG, "no benchmark produced a result")
            if persist:
                store.save_run(measured)
            if baseline and persist:
                store.set_baseline(measured.host, measured.epoch, measured.run_id)
    except store.LockedError:
        die(PROG, "another benchmark is already running")
    if as_json:
        out(json.dumps(measured.to_json(), indent=2))
    else:
        report.render_run(measured)
        if not persist:
            out("  not stored")
    if baseline and persist:
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
        found = _pick_run("choosing a run")
        if found is None:
            return
    if as_json:
        out(json.dumps(found.to_json(), indent=2))
        return
    report.render_run(found)


@app.command("list", help="List stored runs.")
def list_(
    host: Annotated[str, typer.Option("--host", help="Only this machine.")] = "",
    limit: Annotated[int, typer.Option("--limit", help="Rows to print.")] = 20,
    all_grades: Annotated[
        bool, typer.Option("--all", help="Include noisy and aborted runs.")
    ] = False,
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
    name = current_host(host)
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
        _open("compare", "compare without both selectors")
        return
    emit_comparison(require_run(left, left), require_run(right, right), as_json)


@app.command(help="Show one metric over time.")
def trend(
    target: Annotated[str, typer.Argument(help="Selector such as archie.")] = "",
    metric: Annotated[str, typer.Argument(help="Metric key such as cpu.multi.")] = "",
):
    if not target or not metric:
        _open("trend", "trend without a metric")
        return
    selector = select.parse(target)
    runs = [
        run
        for run in store.list_runs(selector.host or None, grades=select.CLEAN)
        if select.matches(run, selector)
    ]
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
            _open("baseline", "baseline set without a selector")
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
    yes: Annotated[bool, typer.Option("--yes", help="Delete without confirming.")] = False,
):
    dropped = store.prunable(host or None, keep=keep)
    if not dropped:
        out("  nothing to prune")
        return
    for run in dropped:
        out(f"  {'would remove' if dry_run else 'remove'} {run.host}/{run.run_id}")
    if dry_run:
        out(f"  {len(dropped)} runs would be removed; re-run without --dry-run")
        return
    # The only irreversible operation here, and it used to delete on sight.
    if not yes:
        if not sys.stdin.isatty():
            die(PROG, f"refusing to remove {len(dropped)} runs unattended; pass --yes")
        if not typer.confirm(f"Remove {len(dropped)} runs?", default=False):
            out("  nothing removed")
            return
    try:
        with store.exclusive():
            for run in dropped:
                store.run_path(run.host, run.run_id).unlink(missing_ok=True)
    except store.LockedError:
        die(PROG, "another benchmark is already running")
    out(f"  removed {len(dropped)} runs")
