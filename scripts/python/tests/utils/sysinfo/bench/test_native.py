import json

import pytest

from tools.utils.sysinfo.bench.suites import MeasurementError, native


def fake_binary(tmp_path, stdout, exit_code=0):
    script = tmp_path / "bench-workloads"
    script.write_text(
        "#!/bin/sh\n"
        'if [ "$1" = --version ]; then echo "bench-workloads 0.1.0"; exit 0; fi\n'
        f"echo '{stdout}'\n"
        f"exit {exit_code}\n"
    )
    script.chmod(0o755)
    return script


def test_missing_binary_yields_no_jobs(monkeypatch, tmp_path):
    monkeypatch.setenv("SYSINFO_BENCH_WORKLOADS", str(tmp_path / "absent"))
    assert native.jobs(None) == []


def test_jobs_measure_through_the_binary(monkeypatch, tmp_path):
    payload = json.dumps({"workload": "cpu", "unit": "Mops/s", "value": 123.4})
    binary = fake_binary(tmp_path, payload)
    monkeypatch.setenv("SYSINFO_BENCH_WORKLOADS", str(binary))
    found = native.jobs(None)
    assert [job.name for job in found] == ["cpu.native", "mem.native"]
    assert all(job.version == "0.1.0" for job in found)
    assert found[0].measure() == {
        "cpu.native_single": 123.4,
        "cpu.native_multi": 123.4,
    }
    assert found[1].measure() == {
        "mem.native_read": 123.4,
        "mem.native_write": 123.4,
    }


def test_unreadable_output_is_a_measurement_error(monkeypatch, tmp_path):
    binary = fake_binary(tmp_path, "not json")
    monkeypatch.setenv("SYSINFO_BENCH_WORKLOADS", str(binary))
    with pytest.raises(MeasurementError):
        native.jobs(None)[0].measure()


def test_missing_value_is_a_measurement_error(monkeypatch, tmp_path):
    binary = fake_binary(tmp_path, json.dumps({"workload": "cpu"}))
    monkeypatch.setenv("SYSINFO_BENCH_WORKLOADS", str(binary))
    with pytest.raises(MeasurementError):
        native.jobs(None)[0].measure()


def test_failing_binary_is_a_measurement_error(monkeypatch, tmp_path):
    binary = fake_binary(tmp_path, json.dumps({"value": 1.0}), exit_code=3)
    monkeypatch.setenv("SYSINFO_BENCH_WORKLOADS", str(binary))
    with pytest.raises(MeasurementError):
        native.jobs(None)[0].measure()
