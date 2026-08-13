from types import SimpleNamespace

from tools.utils.sysinfo.bench import conditions


def test_a_healthy_machine_has_no_gate_reasons():
    reasons = conditions.gate_reasons(
        {"on_battery": False, "loadavg_1": 0.1, "cpu_count": 16, "throttled_at_start": False},
    )

    assert reasons == ()


def test_running_on_battery_blocks_a_run():
    reasons = conditions.gate_reasons({"on_battery": True, "cpu_count": 8})

    assert "running on battery" in reasons


def test_a_busy_machine_blocks_a_run():
    reasons = conditions.gate_reasons({"loadavg_1": 12.0, "cpu_count": 16})

    assert any("load average" in reason for reason in reasons)


def test_a_lightly_loaded_machine_does_not_block():
    assert conditions.gate_reasons({"loadavg_1": 1.0, "cpu_count": 16}) == ()


def test_an_already_throttled_machine_blocks_a_run():
    reasons = conditions.gate_reasons({"throttled_at_start": True, "cpu_count": 8})

    assert "the machine is already thermally throttled" in reasons


def test_free_space_only_matters_when_the_tier_writes():
    values = {"free_disk_ratio": 0.05, "cpu_count": 8}

    assert conditions.gate_reasons(values, writes_disk=False) == ()
    assert any("free" in reason for reason in conditions.gate_reasons(values, writes_disk=True))


def test_a_clean_run_has_metrics_and_no_reasons():
    assert conditions.grade_for((), ["a metric"]) == "clean"


def test_a_forced_run_is_graded_noisy():
    assert conditions.grade_for(("running on battery",), ["a metric"]) == "noisy"


def test_a_run_without_metrics_is_aborted():
    assert conditions.grade_for((), []) == "aborted"


def test_an_inactive_nvidia_throttle_reason_is_not_a_throttle(monkeypatch):
    monkeypatch.setattr(conditions.shutil, "which", lambda name: "/usr/bin/nvidia-smi")
    monkeypatch.setattr(
        conditions,
        "run",
        lambda *args, **kwargs: SimpleNamespace(returncode=0, stdout="Not Active, Not Active\n"),
    )

    assert conditions.nvidia_throttled() is False


def test_an_active_nvidia_throttle_reason_is_a_throttle(monkeypatch):
    monkeypatch.setattr(conditions.shutil, "which", lambda name: "/usr/bin/nvidia-smi")
    monkeypatch.setattr(
        conditions,
        "run",
        lambda *args, **kwargs: SimpleNamespace(returncode=0, stdout="Not Active, Active\n"),
    )

    assert conditions.nvidia_throttled() is True


def test_a_machine_without_nvidia_reports_no_throttle(monkeypatch):
    monkeypatch.setattr(conditions.shutil, "which", lambda name: None)

    assert conditions.nvidia_throttled() is False


def test_a_charging_laptop_is_not_on_battery(macos_snapshot):
    assert conditions.on_battery(macos_snapshot) is False


def test_a_desktop_without_a_battery_is_not_on_battery(workstation_snapshot):
    assert conditions.on_battery(workstation_snapshot) is False
