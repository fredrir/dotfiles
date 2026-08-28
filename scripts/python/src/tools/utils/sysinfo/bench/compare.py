from dataclasses import dataclass

from tools.utils.sysinfo.bench.limits import (
    NOISE_FLOOR_PCT,
    NOISE_MAD_FACTOR,
    NOISE_SINGLE_PCT,
)
from tools.utils.sysinfo.bench.record import (
    HOST,
    LIB,
    PLATFORM,
    WORLD,
    comparable_methods,
    snapshot_differences,
)

BETTER = "better"
WORSE = "worse"
NOISE = "noise"
BLOCKED = "blocked"

WORKLOAD = "workload"
DIRTY = "-dirty"


@dataclass(frozen=True)
class Delta:
    key: str
    scale: str
    proportion: str
    comparable: str
    left: float
    right: float
    change_pct: float
    band_pct: float
    verdict: str
    reason: str = ""


def noise_band(left, right):
    values = []
    unreplicated = False
    for metric in (left, right):
        if metric.times_to_run < 2:
            unreplicated = True
        median = metric.median
        deviation = metric.mad
        if median and deviation is not None:
            values.append(abs(deviation / median) * 100)
    floor = NOISE_SINGLE_PCT if unreplicated else NOISE_FLOOR_PCT
    if not values:
        return floor
    return max(floor, NOISE_MAD_FACTOR * max(values))


def configuration_reason(left_run, right_run):
    before, after = left_run.dotfiles_sha, right_run.dotfiles_sha
    if before and after and before != after:
        return f"configuration changed: {before} vs {after}"
    if before.endswith(DIRTY) or after.endswith(DIRTY):
        return "measured against an uncommitted working tree"
    return ""


def blocking_reason(left_run, right_run, left, right):
    if not comparable_methods(left, right):
        return f"method changed: {left.method} vs {right.method}"
    if left.family == WORKLOAD:
        reason = configuration_reason(left_run, right_run)
        if reason:
            return reason
    if left.comparable == HOST and left_run.host != right_run.host:
        return "metric is only comparable within one machine"
    if left.comparable == PLATFORM and left_run.install.get("os") != right_run.install.get("os"):
        return "metric is only comparable within one platform"
    if left.tool != right.tool:
        return f"different tool: {left.tool} vs {right.tool}"
    if left.comparable == WORLD and left.tool_version != right.tool_version:
        return f"different {left.tool} version: {left.tool_version} vs {right.tool_version}"
    return ""


def compare_metric(left_run, right_run, left, right):
    reason = blocking_reason(left_run, right_run, left, right)
    change = 0.0
    if left.median:
        change = (right.median - left.median) / left.median * 100
    band = noise_band(left, right)
    if reason:
        verdict = BLOCKED
    elif abs(change) <= band:
        verdict = NOISE
    elif (change < 0) == (left.proportion == LIB):
        verdict = BETTER
    else:
        verdict = WORSE
    return Delta(
        key=left.key,
        scale=left.scale,
        proportion=left.proportion,
        comparable=left.comparable,
        left=left.median,
        right=right.median,
        change_pct=change,
        band_pct=band,
        verdict=verdict,
        reason=reason,
    )


def compare_runs(left_run, right_run):
    deltas = []
    for left in left_run.metrics:
        right = right_run.metric(left.key)
        if right is None or left.median is None or right.median is None:
            continue
        deltas.append(compare_metric(left_run, right_run, left, right))
    only_left = tuple(
        metric.key for metric in left_run.metrics if right_run.metric(metric.key) is None
    )
    only_right = tuple(
        metric.key for metric in right_run.metrics if left_run.metric(metric.key) is None
    )
    changes = snapshot_differences(left_run.snapshot, right_run.snapshot)
    return tuple(deltas), changes, only_left, only_right


def regressions(deltas, threshold):
    return tuple(
        delta for delta in deltas if delta.verdict == WORSE and abs(delta.change_pct) >= threshold
    )
