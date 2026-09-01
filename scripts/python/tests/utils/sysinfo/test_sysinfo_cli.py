from types import SimpleNamespace

import pytest
from typer.testing import CliRunner

from tools.utils.sysinfo import cli, collect, hosts
from tools.utils.sysinfo.formatting import capacity, memory_capacity


@pytest.mark.parametrize(
    ("arguments", "pretty", "full", "health"),
    [
        ([], False, False, False),
        (["-p"], True, False, False),
        (["-f"], False, True, False),
        (["-hh"], False, False, True),
        (["-p", "-f"], True, True, False),
        (["-p", "-hh"], True, False, True),
        (["-f", "-hh"], False, True, True),
        (["-p", "-f", "-hh"], True, True, True),
        (["-pf"], True, True, False),
    ],
)
def test_all_flag_combinations(monkeypatch, arguments, pretty, full, health):
    snapshot = object()
    view = object()
    issues = (object(),)
    collected = []
    plain_calls = []
    pretty_calls = []
    monkeypatch.setattr(
        cli, "collect_snapshot", lambda full=False: collected.append(full) or snapshot
    )
    monkeypatch.setattr(cli, "build_view", lambda value: view if value is snapshot else None)
    monkeypatch.setattr(cli, "health_issues", lambda value: issues if value is snapshot else ())
    monkeypatch.setattr(cli, "benchmark_issues", lambda host: ())
    monkeypatch.setattr(cli, "current_host", lambda: "archie")
    monkeypatch.setattr(cli, "render_plain", lambda *values: plain_calls.append(values))
    monkeypatch.setattr(cli, "render_pretty", lambda *values: pretty_calls.append(values))

    result = CliRunner().invoke(cli.app, arguments)

    assert result.exit_code == 0
    assert collected == [full or pretty]
    calls = pretty_calls if pretty else plain_calls
    assert len(calls) == 1
    assert calls[0][0:2] == (view, issues)
    assert calls[0][2].full is full
    assert calls[0][2].health is health
    assert not (plain_calls and pretty_calls)


def test_benchmark_findings_join_the_hardware_findings(monkeypatch):
    snapshot = object()
    hardware = (object(),)
    benchmark = (object(),)
    plain_calls = []
    monkeypatch.setattr(cli, "collect_snapshot", lambda full=False: snapshot)
    monkeypatch.setattr(cli, "build_view", lambda value: object())
    monkeypatch.setattr(cli, "health_issues", lambda value: hardware)
    monkeypatch.setattr(cli, "benchmark_issues", lambda host: benchmark)
    monkeypatch.setattr(cli, "current_host", lambda: "archie")
    monkeypatch.setattr(cli, "render_plain", lambda *values: plain_calls.append(values))

    result = CliRunner().invoke(cli.app, [])

    assert result.exit_code == 0
    assert plain_calls[0][1] == hardware + benchmark


def test_bench_is_reachable_as_a_subcommand():
    result = CliRunner().invoke(cli.app, ["bench", "--help"])

    assert result.exit_code == 0
    for name in ("run", "show", "list", "compare", "trend", "baseline", "prune"):
        assert name in result.output


def test_help_lists_every_alias():
    result = CliRunner().invoke(cli.app, ["--help"])

    assert result.exit_code == 0
    for option in ("--pretty", "-p", "--full", "-f", "--health", "-hh"):
        assert option in result.stdout


def test_capacity_formatting():
    assert capacity(2000398934016) == "2 TB"
    assert capacity(1850000000000) == "1.9 TB"
    assert capacity(512110190592) == "512.1 GB"
    assert capacity(157286400) == "157.3 MB"
    assert capacity(None) == "unknown"
    assert memory_capacity(33538248704) == "32 GB"
    assert memory_capacity(68719476736) == "64 GB"


def test_collect_nvidia_parses_multiple_devices(monkeypatch):
    output = (
        "0, NVIDIA GeForce RTX 5070 Ti, 16303, 2048, 8, 39, 32.5, 300, 2805, 610.43.03\n"
        "1, NVIDIA T400, 4096, 512, 2, 35, N/A, 30, 420, 610.43.03\n"
    )
    monkeypatch.setattr(collect.shutil, "which", lambda _name: "/usr/bin/nvidia-smi")
    monkeypatch.setattr(
        collect,
        "capture",
        lambda *_args, **_kwargs: SimpleNamespace(returncode=0, stdout=output, stderr=""),
    )

    devices, error = collect.collect_nvidia()

    assert error == ""
    assert len(devices) == 2
    assert devices[0]["index"] == 0
    assert devices[0]["memory_total_mib"] == 16303
    assert devices[0]["power_draw"] == 32.5
    assert devices[1]["power_draw"] is None


def test_collect_nvidia_reports_driver_library_mismatch(monkeypatch):
    monkeypatch.setattr(collect.shutil, "which", lambda _name: "/usr/bin/nvidia-smi")
    monkeypatch.setattr(
        collect,
        "capture",
        lambda *_args, **_kwargs: SimpleNamespace(
            returncode=1,
            stdout="",
            stderr="Failed to initialize NVML: Driver/library version mismatch\n",
        ),
    )

    devices, error = collect.collect_nvidia()

    assert devices == []
    assert error == "NVIDIA kernel driver does not match the installed userspace library"


def test_snapshot_only_probes_nvidia_when_present(monkeypatch):
    data = [
        {"type": "OS", "result": {"prettyName": "Ubuntu"}},
        {"type": "CPU", "result": {"cpu": "AMD EPYC", "vendor": "AuthenticAMD"}},
        {"type": "GPU", "result": []},
    ]
    monkeypatch.setattr(collect, "load_hardware_config", dict)
    monkeypatch.setattr(collect, "shell_info", lambda: ("zsh", "5.9"))
    monkeypatch.setattr(collect, "terminal_info", lambda: ("", ""))
    monkeypatch.setattr(collect, "collect_fastfetch", lambda full=False: data)
    monkeypatch.setattr(
        collect,
        "collect_nvidia",
        lambda: pytest.fail("NVIDIA should not be probed on a CPU-only host"),
    )

    snapshot = collect.collect_snapshot()

    assert snapshot.nvidia == ()
    assert snapshot.probe_errors == ()


def test_snapshot_rejects_process_wrappers_as_terminals(monkeypatch):
    data = [
        {"type": "OS", "result": {"prettyName": "Arch Linux"}},
        {"type": "Terminal", "result": {"prettyName": "tailscaled", "processName": "uv"}},
    ]
    monkeypatch.setattr(collect, "load_hardware_config", dict)
    monkeypatch.setattr(collect, "shell_info", lambda: ("zsh", "5.9"))
    monkeypatch.setattr(collect, "terminal_info", lambda: ("", ""))
    monkeypatch.setattr(collect, "collect_fastfetch", lambda full=False: data)

    snapshot = collect.collect_snapshot()

    assert snapshot.terminal_display == "unknown"


def test_hostname_selects_the_matching_host(monkeypatch, tmp_path):
    config = tmp_path / "hosts.dotfile"
    config.write_text(
        "archie {\n"
        "  hostnames = archie\n"
        "  MEMORY = Corsair 32 GB DDR5\n"
        "  CPU_COOLER = Noctua NH-D15\n"
        "}\n"
        "macie {\n"
        "  hostnames = macie\n"
        "  MEMORY = Apple unified memory\n"
        "}\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("SYSINFO_CONFIG", str(config))
    monkeypatch.delenv("SYSINFO_HOST", raising=False)
    monkeypatch.setattr(hosts, "saved_host", lambda: "")
    monkeypatch.setattr(hosts, "local_hostnames", lambda: ("macie",))

    hardware = collect.load_hardware_config()

    assert hardware["memory"] == "Apple unified memory"
    assert hardware["cpu_cooler"] == "not set"


def test_explicit_host_overrides_the_hostname(monkeypatch, tmp_path):
    config = tmp_path / "hosts.dotfile"
    config.write_text(
        "archie {\n  hostnames = archie\n  MEMORY = Corsair 32 GB DDR5\n}\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("SYSINFO_CONFIG", str(config))
    monkeypatch.setenv("SYSINFO_HOST", "archie")
    monkeypatch.setattr(hosts, "local_hostnames", lambda: ("macie",))

    hardware = collect.load_hardware_config()

    assert hardware["memory"] == "Corsair 32 GB DDR5"


def test_an_unknown_machine_reports_no_configured_hardware(monkeypatch, tmp_path):
    config = tmp_path / "hosts.dotfile"
    config.write_text("archie {\n  hostnames = archie\n  CASE = ARCTIC Xtender\n}\n", "utf-8")
    monkeypatch.setenv("SYSINFO_CONFIG", str(config))
    monkeypatch.delenv("SYSINFO_HOST", raising=False)
    monkeypatch.setattr(hosts, "saved_host", lambda: "")
    monkeypatch.setattr(hosts, "local_hostnames", lambda: ("thinkpad-x1",))

    hardware = collect.load_hardware_config()

    assert hardware == hosts.DEFAULT_HARDWARE
