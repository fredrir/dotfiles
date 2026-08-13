"""Run and metric builders for the bench tests.

Deliberately not named conftest: two conftest modules were imported by name
(`from conftest import ...`) with no package around them, so pytest registered
both under the bare module name and the second evicted the first. Any run
naming two test directories failed on an import error.
"""

from datetime import UTC, datetime, timedelta

from tools.utils.sysinfo.bench.record import HIB, WORLD, Metric, Run

SNAPSHOT = {
    "cpu": {"model": "AMD Ryzen 7 9800X3D", "cores_physical": 8, "cores_logical": 16},
    "gpu": [{"name": "NVIDIA GeForce RTX 5070 Ti", "memory_total": 17094934528}],
    "memory": {"total": 34359738368, "modules": 2},
    "disks": [{"name": "KINGSTON SNVS2000G", "size": 2000398934016}],
}


def recent(days=0):
    """A timestamp `days` before now.

    Relative rather than fixed, because the health checks compare against
    STALE_DAYS. Fixtures pinned to the day they were written start failing
    once that many days have passed -- these were set to fire in December.
    """
    moment = datetime.now(UTC).replace(microsecond=0) - timedelta(days=days)
    return moment.isoformat().replace("+00:00", "Z")


def metric(key, samples, proportion=HIB, comparable=WORLD, method=None, version="26.02"):
    return Metric(
        key=key,
        method=method or f"{key}/1.0.0",
        scale="MIPS",
        proportion=proportion,
        comparable=comparable,
        tool="7z",
        tool_version=version,
        samples=tuple(samples),
    )


def build_run(run_id="2026-08-13T09-00-00Z-abcd1234", host="archie", metrics=(), **fields):
    payload = {
        "run_id": run_id,
        "host": host,
        "started": fields.pop("started", recent(1)),
        "tier": fields.pop("tier", "quick"),
        "grade": fields.pop("grade", "clean"),
        "snapshot": fields.pop("snapshot", dict(SNAPSHOT)),
        "install": fields.pop("install", {"os": "arch", "kernel": "7.1.8"}),
        "metrics": tuple(metrics),
    }
    payload.update(fields)
    return Run(**payload)
