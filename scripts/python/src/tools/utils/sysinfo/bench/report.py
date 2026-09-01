from rich.table import Table
from rich.text import Text

from tools.core.console import out, stdout
from tools.utils.sysinfo.bench.compare import BETTER, BLOCKED, NOISE, WORSE
from tools.utils.sysinfo.bench.record import LIB

SPARKS = "▁▂▃▄▅▆▇█"

VERDICT_STYLE = {BETTER: "green", WORSE: "red", NOISE: "dim", BLOCKED: "yellow"}

GRADE_STYLE = {"clean": "green", "noisy": "yellow", "aborted": "red"}


def format_value(value, scale=""):
    if value is None:
        return "—"
    if abs(value) >= 1000:
        text = f"{value:,.0f}".replace(",", " ")
    elif abs(value) >= 10:
        text = f"{value:.1f}"
    else:
        text = f"{value:.2f}"
    return f"{text} {scale}".strip()


def format_change(delta):
    if delta.verdict == BLOCKED:
        return "n/a"
    sign = "+" if delta.change_pct > 0 else ""
    return f"{sign}{delta.change_pct:.1f}%"


def sparkline(values):
    usable = [value for value in values if value is not None]
    if len(usable) < 2:
        return ""
    low, high = min(usable), max(usable)
    if high == low:
        return SPARKS[len(SPARKS) // 2] * len(usable)
    span = high - low
    return "".join(
        SPARKS[min(int((value - low) / span * len(SPARKS)), len(SPARKS) - 1)] for value in usable
    )


def describe_run(run):
    parts = [run.host, run.tier, run.grade]
    if run.install.get("os"):
        parts.insert(1, run.install["os"])
    return "  ".join(part for part in parts if part)


def short_label(run):
    return run.started[:16].replace("T", " ")


def render_run(run):
    out()
    out(f"  {run.run_id}")
    out(f"  {describe_run(run)}  epoch {run.epoch}")
    if run.note:
        out(f"  note: {run.note}")
    if run.dotfiles_sha:
        out(f"  dotfiles: {run.dotfiles_sha}")
    if run.bytes_written:
        out(f"  written: {run.bytes_written / 1024**3:.1f} GiB")
    out()
    table = Table(header_style="bold", box=None, pad_edge=False)
    table.add_column("metric")
    table.add_column("median", justify="right")
    table.add_column("unit")
    table.add_column("n", justify="right")
    table.add_column("rsd", justify="right")
    table.add_column("scope")
    table.add_column("tool")
    for metric in run.metrics:
        table.add_row(
            metric.key,
            format_value(metric.median),
            metric.scale,
            str(metric.times_to_run),
            f"{metric.rsd_pct:.1f}%",
            metric.comparable,
            f"{metric.tool} {metric.tool_version}".strip(),
        )
    stdout.print(table)
    if run.gate_reasons:
        out()
        for reason in run.gate_reasons:
            out(f"  ! {reason}")
    out()


def render_list(runs):
    if not runs:
        out("  no runs recorded")
        return
    table = Table(header_style="bold", box=None, pad_edge=False)
    table.add_column("date", no_wrap=True)
    table.add_column("host")
    table.add_column("os")
    table.add_column("epoch")
    table.add_column("tier")
    table.add_column("grade")
    table.add_column("metrics", justify="right")
    table.add_column("note")
    for run in runs:
        table.add_row(
            short_label(run),
            run.host,
            run.install.get("os", ""),
            run.epoch,
            run.tier,
            Text(run.grade, style=GRADE_STYLE.get(run.grade, "")),
            str(len(run.metrics)),
            (run.note or "")[:32],
        )
    stdout.print(table)


def render_comparison(left, right, deltas, changes, only_left, only_right):
    out()
    out(f"  {left.run_id}   {describe_run(left)}")
    out(f"  {right.run_id}   {describe_run(right)}")
    out()
    if changes:
        for label, field, before, after in changes:
            out(f"  hardware changed: {label} {field}: {before} → {after}")
        out()
    table = Table(header_style="bold", box=None, pad_edge=False)
    table.add_column("metric")
    table.add_column("left", justify="right")
    table.add_column("right", justify="right")
    table.add_column("unit")
    table.add_column("change", justify="right")
    table.add_column("verdict")
    for delta in deltas:
        note = delta.verdict
        if delta.verdict == NOISE:
            note = f"within noise (±{delta.band_pct:.1f}%)"
        table.add_row(
            delta.key,
            format_value(delta.left),
            format_value(delta.right),
            delta.scale,
            format_change(delta),
            Text(note, style=VERDICT_STYLE.get(delta.verdict, "")),
        )
    stdout.print(table)
    blocked = [delta for delta in deltas if delta.verdict == BLOCKED]
    if blocked:
        out()
        for delta in blocked:
            out(f"  ! {delta.key}: {delta.reason}")
    if only_left or only_right:
        out()
        if only_left:
            out(f"  only on the left: {', '.join(only_left)}")
        if only_right:
            out(f"  only on the right: {', '.join(only_right)}")
    out()
    out()


def render_trend(runs, key):
    ordered = sorted(runs, key=lambda run: run.started)
    rows = [(run, run.metric(key)) for run in ordered]
    rows = [(run, metric) for run, metric in rows if metric and metric.median is not None]
    if not rows:
        out(f"  no runs carry {key}")
        return
    scale = rows[0][1].scale
    proportion = rows[0][1].proportion
    out()
    out(f"  {key}   {scale}   {'lower is better' if proportion == LIB else 'higher is better'}")
    out()
    table = Table(header_style="bold", box=None, pad_edge=False)
    table.add_column("date")
    table.add_column("epoch")
    table.add_column("median", justify="right")
    table.add_column("rsd", justify="right")
    table.add_column("note")
    for run, metric in rows:
        table.add_row(
            run.started[:10],
            run.epoch,
            format_value(metric.median),
            f"{metric.rsd_pct:.1f}%",
            (run.note or "")[:28],
        )
    stdout.print(table)
    spark = sparkline([metric.median for _run, metric in rows])
    if spark:
        out()
        out(f"  {spark}")
    out()
