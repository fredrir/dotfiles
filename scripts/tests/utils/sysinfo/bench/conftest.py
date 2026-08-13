import pytest

from tools.utils.sysinfo.bench.record import HIB, LIB, WORLD, Metric, Run

SNAPSHOT = {
    "cpu": {"model": "AMD Ryzen 7 9800X3D", "cores_physical": 8, "cores_logical": 16},
    "gpu": [{"name": "NVIDIA GeForce RTX 5070 Ti", "memory_total": 17094934528}],
    "memory": {"total": 34359738368, "modules": 2},
    "disks": [{"name": "KINGSTON SNVS2000G", "size": 2000398934016}],
}


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
        "started": fields.pop("started", "2026-08-13T09:00:00Z"),
        "tier": fields.pop("tier", "quick"),
        "grade": fields.pop("grade", "clean"),
        "snapshot": fields.pop("snapshot", dict(SNAPSHOT)),
        "install": fields.pop("install", {"os": "arch", "kernel": "7.1.8"}),
        "metrics": tuple(metrics),
    }
    payload.update(fields)
    return Run(**payload)


@pytest.fixture
def benchmarks(tmp_path, monkeypatch):
    monkeypatch.setenv("SYSINFO_BENCHMARKS", str(tmp_path / "benchmarks"))
    return tmp_path / "benchmarks"


@pytest.fixture
def sample_run():
    return build_run(metrics=(metric("cpu.multi", [100.0, 101.0, 99.0]),))


@pytest.fixture
def slower_run():
    return build_run(
        run_id="2026-08-14T09-00-00Z-abcd1234",
        started="2026-08-14T09:00:00Z",
        metrics=(metric("cpu.multi", [70.0, 71.0, 69.0]),),
    )


@pytest.fixture
def latency_metric():
    return metric("workload.nvim_startup", [20.0, 21.0, 19.0], proportion=LIB)
