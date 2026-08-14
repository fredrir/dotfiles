import json

import pytest

from tools.utils.sysinfo import collect


def fake_collector(tmp_path, stdout, exit_code=0):
    script = tmp_path / "sysinfo-collect"
    script.write_text(f"#!/bin/sh\necho '{stdout}'\nexit {exit_code}\n")
    script.chmod(0o755)
    return script


def modules(*pairs):
    return json.dumps([{"type": kind, "result": result} for kind, result in pairs])


def test_native_output_is_used_without_fastfetch(monkeypatch, tmp_path):
    binary = fake_collector(tmp_path, modules(("CPU", {"cpu": "Test CPU"})))
    monkeypatch.setenv("SYSINFO_COLLECTOR", str(binary))
    monkeypatch.setattr(collect, "collect_fastfetch", lambda full=False: pytest.fail("fell back"))
    found = collect.collect_modules()
    assert found == [{"type": "CPU", "result": {"cpu": "Test CPU"}}]


def test_missing_binary_falls_back_to_fastfetch(monkeypatch, tmp_path):
    monkeypatch.setenv("SYSINFO_COLLECTOR", str(tmp_path / "absent"))
    monkeypatch.setattr(collect, "collect_fastfetch", lambda full=False: [{"type": "OS"}])
    assert collect.collect_modules() == [{"type": "OS"}]


def test_failing_binary_falls_back_to_fastfetch(monkeypatch, tmp_path):
    binary = fake_collector(tmp_path, modules(("CPU", {})), exit_code=1)
    monkeypatch.setenv("SYSINFO_COLLECTOR", str(binary))
    monkeypatch.setattr(collect, "collect_fastfetch", lambda full=False: [{"type": "OS"}])
    assert collect.collect_modules() == [{"type": "OS"}]


def test_unreadable_output_falls_back_to_fastfetch(monkeypatch, tmp_path):
    binary = fake_collector(tmp_path, "not json")
    monkeypatch.setenv("SYSINFO_COLLECTOR", str(binary))
    monkeypatch.setattr(collect, "collect_fastfetch", lambda full=False: [{"type": "OS"}])
    assert collect.collect_modules() == [{"type": "OS"}]


def test_full_mode_merges_only_the_missing_modules(monkeypatch, tmp_path):
    binary = fake_collector(tmp_path, modules(("CPU", {"cpu": "Test CPU"}), ("OS", {})))
    monkeypatch.setenv("SYSINFO_COLLECTOR", str(binary))
    asked = {}

    def fake_fastfetch(wanted):
        asked["modules"] = [collect.module_name(module) for module in wanted]
        return [{"type": "Host", "result": {"name": "box"}}]

    monkeypatch.setattr(collect, "fastfetch_modules", fake_fastfetch)
    found = collect.collect_modules(full=True)
    assert {"type": "Host", "result": {"name": "box"}} in found
    assert "CPU" not in asked["modules"]
    assert "OS" not in asked["modules"]
    assert "Host" in asked["modules"]
    assert "Display" in asked["modules"]


def test_full_mode_survives_a_missing_fastfetch(monkeypatch, tmp_path):
    native = modules(("CPU", {"cpu": "Test CPU"}))
    binary = fake_collector(tmp_path, native)
    monkeypatch.setenv("SYSINFO_COLLECTOR", str(binary))
    monkeypatch.setattr(collect, "fastfetch_modules", lambda wanted: None)
    assert collect.collect_modules(full=True) == json.loads(native)
