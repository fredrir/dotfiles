from dataclasses import replace

from tools.utils.sysinfo.health import health_issues, health_summary
from tools.utils.sysinfo.view import build_view


def test_view_normalizes_current_hardware(workstation_snapshot):
    view = build_view(workstation_snapshot)
    components = {(component.label, component.model): component for component in view.components}

    assert ("CPU", "Ryzen 7 9800X3D") in components
    assert ("GPU", "GeForce RTX 5070 Ti") in components
    assert ("MEMORY", "32 GB  DDR5-6000  CL30") in components
    assert ("MOTHERBOARD", "B850-PLUS WIFI") in components
    assert ("STORAGE", "NV1  2 TB  SSD") in components
    assert ("STORAGE", "WD Blue  2 TB  HDD") in components
    integrated = next(
        component for component in view.components if component.label == "INTEGRATED GPU"
    )
    assert integrated.compact is False
    assert "PRIVATE" not in repr(view)


def test_view_normalizes_software(workstation_snapshot):
    view = build_view(workstation_snapshot)

    assert view.platform.label == "Arch Linux"
    assert view.machine_type == "WORKSTATION"
    assert view.summary == ("Arch Linux", "KDE Plasma", "Wayland")
    assert [badge.kind for badge in view.software] == [
        "desktop",
        "wm",
        "session",
        "terminal",
        "shell",
    ]


def test_health_detects_pressure_and_component_limits(workstation_snapshot):
    modules = dict(workstation_snapshot.modules)
    modules["Memory"] = {"used": 31 * 1024**3, "total": 32 * 1024**3}
    modules["Disk"] = [
        {
            "mountpoint": "/",
            "bytes": {"used": 95 * 1024**3, "total": 100 * 1024**3},
        }
    ]
    hardware = dict(workstation_snapshot.hardware, memory="Corsair 64 GB DDR5")
    nvidia = (dict(workstation_snapshot.nvidia[0], temperature=90.0),)
    snapshot = replace(
        workstation_snapshot,
        hardware=hardware,
        modules=modules,
        nvidia=nvidia,
    )

    issues = health_issues(snapshot)
    text = "\n".join(f"{issue.title}\n{issue.detail}" for issue in issues)

    assert len(issues) == 4
    assert "Configured as 64 GB, detected as 32 GB" in text
    assert "Memory is 97% used and swap is disabled" in text
    assert "90°C measured, 88°C maximum" in text
    assert "95% of the filesystem is used" in text
    assert any(issue.severity == "error" for issue in issues)
    assert health_summary(issues) == "2 errors  2 warnings"


def test_swap_is_not_advice_without_memory_pressure(workstation_snapshot):
    issues = health_issues(workstation_snapshot)

    assert not any("swap" in f"{issue.title} {issue.detail}".lower() for issue in issues)


def test_probe_mismatch_is_actionable_error(workstation_snapshot):
    snapshot = replace(
        workstation_snapshot,
        probe_errors=("NVIDIA kernel driver does not match the installed userspace library",),
    )

    issues = health_issues(snapshot)

    assert issues[0].severity == "error"
    assert issues[0].action == "Reboot to load the updated NVIDIA kernel module"


def test_low_battery_is_only_reported_when_discharging(workstation_snapshot):
    modules = dict(workstation_snapshot.modules)
    modules["Battery"] = [{"capacity": 12.0, "status": "Discharging"}]
    discharging = replace(workstation_snapshot, modules=modules)
    modules = dict(modules)
    modules["Battery"] = [{"capacity": 12.0, "status": "Charging"}]
    charging = replace(workstation_snapshot, modules=modules)

    assert any(issue.title == "Battery charge is low" for issue in health_issues(discharging))
    assert not any(issue.title == "Battery charge is low" for issue in health_issues(charging))


def test_macos_view_rejects_desktop_profile_and_virtual_devices(macos_snapshot):
    view = build_view(macos_snapshot)
    components = {component.label: [] for component in view.components}
    for component in view.components:
        components[component.label].append(component)

    assert view.platform.label == "macOS 26.0"
    assert view.machine_type == "WORKSTATION"
    assert components["MEMORY"][0].vendor == "APPLE"
    assert components["MEMORY"][0].model == "24 GB"
    assert len(components["STORAGE"]) == 1
    assert components["STORAGE"][0].model == "SSD AP1024Z Media  1 TB  SSD"
    assert components["BATTERY"][0].model == "Internal battery"
    assert components["BATTERY"][0].facts[1].value == "AC Connected"
    assert components["POWER ADAPTER"][0].model == "70 W"
    assert components["POWER ADAPTER"][0].facts == ()
    assert not any(
        component.label in {"CPU COOLING", "CHASSIS", "POWER SUPPLY"}
        for component in view.components
    )
    assert health_issues(macos_snapshot) == ()
