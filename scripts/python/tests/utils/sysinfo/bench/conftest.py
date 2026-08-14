import pytest
from builders import build_run, metric, recent

from tools.utils.sysinfo.bench.record import LIB


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
        started=recent(0),
        metrics=(metric("cpu.multi", [70.0, 71.0, 69.0]),),
    )


@pytest.fixture
def latency_metric():
    return metric("workload.nvim_startup", [20.0, 21.0, 19.0], proportion=LIB)
