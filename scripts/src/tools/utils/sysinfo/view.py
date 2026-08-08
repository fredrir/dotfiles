from tools.utils.sysinfo.formatting import as_dict
from tools.utils.sysinfo.hardware import hardware_components
from tools.utils.sysinfo.models import SystemView
from tools.utils.sysinfo.software import software_badges, system_facts


def build_view(snapshot):
    platform, software = software_badges(snapshot)
    desktop_detected = bool(snapshot.result("DE", {}) or snapshot.result("WM", {}))
    workstation_configured = any(
        snapshot.hardware.get(key) not in (None, "", "not set")
        for key in ("cpu_cooler", "case", "power_supply")
    )
    summary = [platform.label]
    de = as_dict(snapshot.result("DE", {}))
    if snapshot.de_display != "unknown":
        summary.append(de.get("prettyName") or snapshot.de_display)
    wm = as_dict(snapshot.result("WM", {}))
    if wm.get("protocolName"):
        summary.append(wm["protocolName"])
    return SystemView(
        platform=platform,
        machine_type="WORKSTATION" if desktop_detected or workstation_configured else "SERVER",
        summary=tuple(summary),
        components=hardware_components(snapshot),
        software=software,
        system_facts=system_facts(snapshot),
    )
