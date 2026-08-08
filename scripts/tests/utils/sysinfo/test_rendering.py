from io import StringIO

from rich.console import Console

from tools.utils.sysinfo.models import HealthIssue, RenderOptions
from tools.utils.sysinfo.plain import render_plain
from tools.utils.sysinfo.pretty import render_pretty
from tools.utils.sysinfo.view import build_view


def pretty_output(
    monkeypatch,
    snapshot,
    width=120,
    full=False,
    health=False,
    issues=(),
    hostname="archpc",
):
    monkeypatch.setattr("tools.utils.sysinfo.pretty.display_username", lambda: "fredrir")
    monkeypatch.setattr("tools.utils.sysinfo.pretty.display_hostname", lambda: hostname)
    stream = StringIO()
    console = Console(
        file=stream,
        width=width,
        no_color=True,
        force_terminal=False,
        highlight=False,
    )
    render_pretty(
        build_view(snapshot),
        tuple(issues),
        RenderOptions(full=full, health=health),
        console=console,
    )
    return stream.getvalue()


def test_pretty_is_borderless_branded_and_hardware_complete(
    monkeypatch,
    workstation_snapshot,
):
    output = pretty_output(monkeypatch, workstation_snapshot)

    assert "fredrir@archpc" not in output
    assert "FREDRIR   WORKSTATION" in output
    assert "ARCHPC" in output
    assert "█" in output
    assert output.splitlines().index("HARDWARE") >= 12
    for expected in (
        "AMD",
        "NVIDIA",
        "CORSAIR",
        "ASUS TUF",
        "KINGSTON",
        "WESTERN DIGITAL",
        "ARCH LINUX",
        "KDE PLASMA",
        "KWIN",
        "WAYLAND",
        "8 cores / 16 threads",
        "96 MB L3",
        "120 W TDP",
        "PCIe 5.0 ×16",
        "Driver",
        "NOCTUA",
        "ARCTIC",
        "POWER SUPPLY",
    ):
        assert expected in output
    for hidden in (
        "SOFTWARE",
        "SYSTEM",
        "Konsole",
        "zsh",
        "tailscaled",
        "no active warnings",
        "no spillover",
        "PRIVATE",
    ):
        assert hidden not in output
    assert chr(183) not in output
    assert not output.startswith("╭")
    assert max(len(line) for line in output.splitlines()) <= 120


def test_full_pretty_adds_software_and_system_without_health_prose(
    monkeypatch,
    workstation_snapshot,
):
    issues = (
        HealthIssue(
            "warning",
            "A synthetic warning",
            "Detailed diagnostic text",
            "Take a synthetic action",
        ),
    )
    output = pretty_output(
        monkeypatch,
        workstation_snapshot,
        full=True,
        health=False,
        issues=issues,
    )

    for expected in (
        "8 cores / 16 threads",
        "96 MB L3",
        "120 W TDP",
        "PCIe 5.0 ×16",
        "Driver",
        "NOCTUA",
        "ARCTIC",
        "POWER SUPPLY",
        "1 warning",
        "KDE Plasma 6.7.3",
        "Konsole 26.04.3",
        "SOFTWARE",
        "SYSTEM",
        "Kernel",
        "Packages",
        "OpenCL",
        "Vulkan",
    ):
        assert expected in output
    assert "Detailed diagnostic text" not in output
    assert "Take a synthetic action" not in output
    assert chr(183) not in output


def test_health_flag_reveals_only_active_findings(monkeypatch, workstation_snapshot):
    issue = HealthIssue(
        "error",
        "NVIDIA driver mismatch",
        "The kernel module is older than userspace",
        "Reboot to load the updated module",
    )
    output = pretty_output(
        monkeypatch,
        workstation_snapshot,
        health=True,
        issues=(issue,),
    )

    assert "1 error" in output
    assert "HEALTH" in output
    assert "NVIDIA driver mismatch" in output
    assert "Reboot to load the updated module" in output

    healthy = pretty_output(monkeypatch, workstation_snapshot, health=True)
    assert "HEALTH" not in healthy
    assert "warning" not in healthy.lower()


def test_narrow_pretty_stacks_without_overflow(monkeypatch, workstation_snapshot):
    output = pretty_output(monkeypatch, workstation_snapshot, width=52)

    assert output.startswith("FREDRIR   WORKSTATION")
    assert output.index("AMD") < output.index("NVIDIA") < output.index("CORSAIR")
    assert max(len(line) for line in output.splitlines()) <= 52


def test_macos_pretty_uses_native_hardware_and_clean_power_details(
    monkeypatch,
    macos_snapshot,
):
    output = pretty_output(
        monkeypatch,
        macos_snapshot,
        hostname="fredrirs-macbook-pro",
    )

    for expected in (
        "FREDRIR   WORKSTATION",
        "FREDRIRS-MACBOOK-PRO",
        "MACOS",
        "APPLE  CPU",
        "APPLE  INTEGRATED GPU",
        "APPLE  MEMORY",
        "APPLE  MOTHERBOARD",
        "APPLE  STORAGE",
        "APPLE  BATTERY",
        "APPLE  POWER ADAPTER",
        "24 GB",
        "Internal battery",
        "AC Connected",
        "70 W",
    ):
        assert expected in output
    for rejected in (
        "bq40z651",
        "['AC Connected']",
        "Apple Disk Image Media",
        "CORSAIR",
        "NOCTUA",
        "ARCTIC",
        "RM1000e",
        "STORAGE  STORAGE",
        "POWER  POWER ADAPTER",
        "warning",
    ):
        assert rejected not in output
    assert "▄██████▄" in output
    assert "MACBOOK-PRO" not in output.splitlines()[2]
    assert "█" in output.splitlines()[2]
    assert output.count("APPLE  STORAGE") == 1
    assert max(len(line) for line in output.splitlines()) <= 120


def test_truecolor_uses_brand_accents(monkeypatch, workstation_snapshot):
    monkeypatch.setattr("tools.utils.sysinfo.pretty.display_username", lambda: "fredrir")
    monkeypatch.setattr("tools.utils.sysinfo.pretty.display_hostname", lambda: "archpc")
    stream = StringIO()
    console = Console(
        file=stream,
        width=120,
        color_system="truecolor",
        force_terminal=True,
        no_color=False,
        highlight=False,
    )

    render_pretty(
        build_view(workstation_snapshot),
        (),
        RenderOptions(),
        console=console,
    )

    output = stream.getvalue()
    assert "38;2;237;28;36m" in output
    assert "38;2;118;185;0m" in output


def test_plain_modes_share_the_same_information_hierarchy(capsys, workstation_snapshot):
    view = build_view(workstation_snapshot)
    issue = HealthIssue(
        "warning",
        "Storage pressure",
        "The root filesystem is nearly full",
        "Remove unused files",
    )

    render_plain(view, (issue,), RenderOptions())
    compact = capsys.readouterr().out
    assert "CPU: AMD Ryzen 7 9800X3D" in compact
    assert "INTEGRATED GPU" not in compact
    assert "Cores:" not in compact
    assert "Health: 1 warning" in compact
    assert "The root filesystem" not in compact

    render_plain(view, (issue,), RenderOptions(full=True))
    full = capsys.readouterr().out
    assert "Cores: 8 cores / 16 threads" in full
    assert "INTEGRATED GPU" in full
    assert "The root filesystem" not in full

    render_plain(view, (issue,), RenderOptions(health=True))
    health = capsys.readouterr().out
    assert "Warning: Storage pressure" in health
    assert "The root filesystem is nearly full" in health
    assert "Action: Remove unused files" in health
    assert chr(183) not in compact + full + health


def test_no_legacy_separator_exists_in_sysinfo_source():
    from pathlib import Path

    root = Path(__file__).parents[3] / "src" / "tools" / "utils" / "sysinfo"
    content = "\n".join(path.read_text(encoding="utf-8") for path in root.glob("*.py"))

    assert chr(183) not in content
