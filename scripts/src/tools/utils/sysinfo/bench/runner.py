import os
import shutil
import subprocess
import time
from dataclasses import dataclass
from datetime import UTC, datetime

from tools.core.process import capture as run
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
    thermal,
    workload,
)
from tools.utils.sysinfo.collect import collect_snapshot

SUITES = (cpu, memory, disk, gpu, thermal, workload)

FAMILIES = ("cpu", "mem", "disk", "gpu", "thermal", "workload")


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


def filesystem_of(path):
    if not shutil.which("findmnt"):
        return {}
    result = run(["findmnt", "-no", "FSTYPE,SOURCE,TARGET", "--target", path])
    if result.returncode != 0:
        return {}
    fields = result.stdout.split()
    if len(fields) < 3:
        return {}
    return {"fstype": fields[0], "source": fields[1], "target": fields[2]}


def collect_jobs(setting):
    found = []
    for module in SUITES:
        found.extend(module.jobs(setting))
    if setting.families:
        found = [job for job in found if job.outputs and family_of(job) in setting.families]
    return found


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
    jobs = collect_jobs(setting)
    metrics = []
    failures = []
    written = 0
    interrupted = False
    try:
        for position, job in enumerate(jobs, start=1):
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
            written += job.writes + int(max(collected.pop(WRITTEN, [0.0]), default=0.0))
            produced = metrics_for(job, collected)
            metrics.extend(produced)
            if report:
                samples = max((metric.times_to_run for metric in produced), default=0)
                report("done", job.name, f"{time.monotonic() - began:.0f}s, n={samples}")
    except KeyboardInterrupt:
        interrupted = True
    grade = "aborted" if interrupted else grade_for(reasons, metrics)
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
