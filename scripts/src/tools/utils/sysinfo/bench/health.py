from datetime import UTC, datetime

from tools.utils.sysinfo.bench import store
from tools.utils.sysinfo.bench.compare import WORSE, compare_runs
from tools.utils.sysinfo.bench.limits import REGRESSION_PCT
from tools.utils.sysinfo.models import HealthIssue

STALE_DAYS = 120


def parse_started(value):
    try:
        return datetime.fromisoformat(value)
    except ValueError:
        return None


def age_in_days(run):
    started = parse_started(run.started)
    if started is None:
        return None
    return (datetime.now(UTC) - started).days


def format_value(value, scale):
    if value is None:
        return "unknown"
    if abs(value) >= 100:
        return f"{value:,.0f} {scale}".replace(",", " ")
    return f"{value:.2f} {scale}"


def regression_issue(delta, baseline, latest):
    return HealthIssue(
        "warning",
        f"{delta.key} is {abs(delta.change_pct):.0f}% below its baseline",
        f"{format_value(delta.left, delta.scale)} at the baseline of {baseline.started[:10]}, "
        f"{format_value(delta.right, delta.scale)} on {latest.started[:10]}",
        f"Re-run sysinfo bench run --only {delta.key.split('.', 1)[0]} to confirm, "
        "then look for thermal or configuration causes",
    )


def benchmark_issues(host):
    if not host:
        return ()
    runs = store.list_runs(host, grades=("clean",))
    if not runs:
        return ()
    latest = runs[0]
    issues = []
    age = age_in_days(latest)
    if age is not None and age >= STALE_DAYS:
        issues.append(
            HealthIssue(
                "warning",
                "The benchmark history for this machine is stale",
                f"The last clean run was {age} days ago",
                "Run sysinfo bench run to refresh the series",
            )
        )
    baseline = store.baseline_run(host, latest.epoch)
    if baseline is None or baseline.run_id == latest.run_id:
        return tuple(issues)
    deltas, _changes, _left, _right = compare_runs(baseline, latest)
    for delta in deltas:
        if delta.verdict == WORSE and abs(delta.change_pct) >= REGRESSION_PCT:
            issues.append(regression_issue(delta, baseline, latest))
    return tuple(issues)
