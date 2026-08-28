import os
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import UTC, datetime

from tools.utils.sysinfo.bench import capture as environment
from tools.utils.sysinfo.bench.conditions import capture_conditions, gate_reasons, grade_for
from tools.utils.sysinfo.bench.limits import (
    COOLDOWN_INTERVAL,
    COOLDOWN_SECONDS,
    MAX_RUNS,
    MIN_RUNS,
    RSD_THRESHOLD,
    WRITE_BUDGET,
)
from tools.utils.sysinfo.bench.record import Metric, Run, epoch_of, relative_deviation
from tools.utils.sysinfo.bench.suites import (
    WRITTEN,
    MeasurementError,
    cpu,
    disk,
    gpu,
    memory,
    native,
    thermal,
    workload,
)
from tools.utils.sysinfo.collect import collect_snapshot

# thermal runs last because it saturates every core for up to two minutes.
# Ahead of workload it meant nvim startup and git status were timed on a hot,
# potentially clock-limited CPU -- the metrics most sensitive to thermal state.
SUITES = (cpu, native, memory, disk, gpu, workload, thermal)

FAMILIES = ("cpu", "mem", "cache", "disk", "gpu", "thermal", "workload")


class GateError(Exception):
    def __init__(self, reasons):
        super().__init__("; ".join(reasons))
        self.reasons = tuple(reasons)


@dataclass(frozen=True)
class Setting:
    tier: str
    workdir: str
    families: tuple[str, ...] = ()


def default_workdir():
    base = os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache")
    path = os.path.join(base, "dotfile", "bench", "work")
    os.makedirs(path, exist_ok=True)
    return path


def under(target, mount):
    return target == mount or target.startswith(mount.rstrip("/") + "/")


def darwin_filesystem(path):
    result = environment.probe(["/sbin/mount"], timeout=10)
    if result is None or result.returncode != 0:
        return {}
    target = os.path.realpath(path)
    found = {}
    longest = -1
    for line in result.stdout.splitlines():
        source, separator, rest = line.partition(" on ")
        mount, _, options = rest.partition(" (")
        mount = mount.strip()
        if not separator or not mount or not under(target, mount):
            continue
        if len(mount) > longest:
            longest = len(mount)
            found = {
                "fstype": options.split(",")[0].strip(" )"),
                "source": source.strip(),
                "target": mount,
            }
    return found


def filesystem_of(path):
    if sys.platform == "darwin":
        return darwin_filesystem(path)
    if not shutil.which("findmnt"):
        return {}
    result = environment.probe(["findmnt", "-no", "FSTYPE,SOURCE,TARGET", "--target", path], 10)
    if result is None or result.returncode != 0:
        return {}
    fields = result.stdout.split()
    if len(fields) < 3:
        return {}
    return {"fstype": fields[0], "source": fields[1], "target": fields[2]}


def suite_name(module):
    return module.__name__.rsplit(".", 1)[-1]


def collect_jobs(setting):
    found = []
    failures = []
    for module in SUITES:
        try:
            found.extend(module.jobs(setting))
        except (MeasurementError, OSError, subprocess.SubprocessError, ValueError) as error:
            failures.append(f"{suite_name(module)}: {error}")
    if setting.families:
        found = [job for job in found if job.outputs and family_of(job) in setting.families]
    return found, tuple(failures)


def family_of(job):
    return job.outputs[0].key.split(".", 1)[0]


def converged(collected):
    if not collected:
        return False
    return all(relative_deviation(values) <= RSD_THRESHOLD for values in collected.values())


def measure_job(job):
    collected = {}
    minimum = MIN_RUNS if job.repeat else 1
    limit = MAX_RUNS if job.repeat else 1
    attempts = 0
    while attempts < limit:
        values = job.measure()
        attempts += 1
        for key, value in values.items():
            bucket = collected.setdefault(key, [])
            if isinstance(value, (list, tuple)):
                bucket.extend(float(item) for item in value)
            else:
                bucket.append(float(value))
        if attempts >= minimum and converged(collected):
            break
    return collected


def metrics_for(job, collected):
    found = []
    for output in job.outputs:
        samples = collected.get(output.key)
        if not samples:
            continue
        found.append(
            Metric(
                key=output.key,
                method=job.method,
                scale=output.scale,
                proportion=output.proportion,
                comparable=output.comparable,
                tool=job.tool,
                tool_version=job.version,
                samples=tuple(samples),
                detail=dict(job.detail),
            )
        )
    return found


def timestamp():
    return datetime.now(UTC).replace(microsecond=0)


def run_id_for(started, snapshot):
    return f"{started.strftime('%Y-%m-%dT%H-%M-%SZ')}-{epoch_of(snapshot)}"


def cool_down(workdir, report):
    deadline = time.monotonic() + COOLDOWN_SECONDS
    while time.monotonic() < deadline:
        if report:
            report("cool", "cooling", f"{int(deadline - time.monotonic())}s left")
        time.sleep(COOLDOWN_INTERVAL)
        snapshot = collect_snapshot(full=True)
        values = capture_conditions(snapshot, workdir)
        if not values["throttled_at_start"]:
            return snapshot, values
    return None, None


def settle(snapshot, conditions, workdir, force, report):
    if force or not conditions.get("throttled_at_start"):
        return snapshot, conditions
    cooled_snapshot, cooled = cool_down(workdir, report)
    if cooled is None:
        return snapshot, conditions
    return cooled_snapshot, cooled


def execute(host, tier, families=(), note="", tags=(), force=False, workdir="", report=None):
    workdir = workdir or default_workdir()
    setting = Setting(tier=tier, workdir=workdir, families=tuple(families))
    snapshot = collect_snapshot(full=True)
    conditions = capture_conditions(snapshot, workdir)
    snapshot, conditions = settle(snapshot, conditions, workdir, force, report)
    conditions["filesystem"] = filesystem_of(workdir)
    writes = WRITE_BUDGET.get(tier, 0) > 0
    reasons = gate_reasons(conditions, writes_disk=writes)
    if reasons and not force:
        raise GateError(reasons)
    described = environment.describe_snapshot(snapshot)
    started = timestamp()
    jobs, failures = collect_jobs(setting)
    failures = list(failures)
    budget = WRITE_BUDGET.get(tier, 0)
    metrics = []
    written = 0
    interrupted = False
    try:
        for position, job in enumerate(jobs, start=1):
            if job.writes and written + job.writes > budget:
                over = (written + job.writes) / 1024**3
                refused = f"would write {over:.1f} GiB, past the {tier} budget"
                failures.append(f"{job.name}: {refused}")
                if report:
                    report("skip", job.name, refused)
                continue
            if report:
                report("start", job.name, f"{position} of {len(jobs)}")
            began = time.monotonic()
            try:
                collected = measure_job(job)
            except (MeasurementError, OSError, subprocess.SubprocessError, ValueError) as error:
                failures.append(f"{job.name}: {error}")
                if report:
                    report("skip", job.name, str(error))
                continue
            # fio's own io_bytes excludes the ramp, so it under-reports; the
            # predicted figure is exact now that the write stages are size bounded.
            measured = int(max(collected.pop(WRITTEN, [0.0]), default=0.0))
            written += max(job.writes, measured)
            produced = metrics_for(job, collected)
            metrics.extend(produced)
            if report:
                samples = max((metric.times_to_run for metric in produced), default=0)
                report("done", job.name, f"{time.monotonic() - began:.0f}s, n={samples}")
    except KeyboardInterrupt:
        interrupted = True
    grade = "aborted" if interrupted else grade_for(reasons, metrics, failures)
    return Run(
        run_id=run_id_for(started, described),
        host=host,
        started=started.isoformat().replace("+00:00", "Z"),
        tier=tier,
        grade=grade,
        snapshot=described,
        install=environment.describe_install(snapshot),
        conditions=conditions,
        metrics=tuple(metrics),
        note=note,
        tags=tuple(tags),
        dotfiles_sha=environment.dotfiles_sha(),
        gate_reasons=tuple(reasons) + tuple(failures),
        bytes_written=written,
    )
