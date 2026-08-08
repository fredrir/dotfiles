from tools.utils.sysinfo.facts import fact, facts
from tools.utils.sysinfo.formatting import (
    as_dict,
    as_list,
    format_bytes,
    format_duration,
    join_parts,
    percentage,
)
from tools.utils.sysinfo.models import SoftwareBadge


def package_text(packages):
    managers = [
        f"{value:,} {name}"
        for name, value in packages.items()
        if name != "all" and isinstance(value, int) and value
    ]
    if managers:
        return join_parts(managers, ", ")
    total = packages.get("all")
    return f"{total:,}" if isinstance(total, int) else ""


def system_facts(snapshot):
    kernel = as_dict(snapshot.result("Kernel", {}))
    host = as_dict(snapshot.result("Host", {}))
    uptime = as_dict(snapshot.result("Uptime", {}))
    bios = as_dict(snapshot.result("BIOS", {}))
    boot = as_dict(snapshot.result("Bootmgr", {}))
    init = as_dict(snapshot.result("InitSystem", {}))
    host_name = join_parts([host.get("vendor"), host.get("family"), host.get("name")])
    secure_boot = ""
    if boot:
        secure_boot = "On" if boot.get("secureBoot") else "Off"
    values = list(
        facts(
            fact("Host", host_name),
            fact("Kernel", join_parts([kernel.get("release"), kernel.get("architecture")])),
            fact("Uptime", format_duration(uptime.get("uptime")) if uptime else ""),
            fact("Packages", package_text(as_dict(snapshot.result("Packages", {})))),
            fact("Firmware", join_parts([bios.get("type"), bios.get("version")])),
            fact("Boot manager", boot.get("name") or ""),
            fact("Secure Boot", secure_boot),
            fact("Init", join_parts([init.get("name"), init.get("version")])),
        )
    )
    opencl = as_dict(snapshot.result("OpenCL", {}))
    vulkan = as_dict(snapshot.result("Vulkan", {}))
    values.extend(
        facts(
            fact("OpenCL", join_parts([opencl.get("name"), opencl.get("version")])),
            fact("Vulkan", join_parts([vulkan.get("apiVersion"), vulkan.get("driver")])),
        )
    )
    theme = as_dict(snapshot.result("Theme", {}))
    theme_name = theme.get("theme1") or snapshot.result("WMTheme", "")
    terminal_font = snapshot.result("TerminalFont", {})
    if isinstance(terminal_font, dict):
        font_name = terminal_font.get("font") or terminal_font.get("name") or ""
    else:
        font_name = ""
    values.extend(facts(fact("Theme", theme_name), fact("Terminal font", font_name)))
    values.extend(filesystem_facts(snapshot))
    values.extend(display_facts(snapshot))
    return tuple(values)


def filesystem_facts(snapshot):
    values = []
    for disk in as_list(snapshot.result("Disk", [])):
        byte_values = as_dict(disk.get("bytes"))
        total = byte_values.get("total") or 0
        used = byte_values.get("used") or 0
        name = disk.get("mountpoint") or disk.get("name") or "Filesystem"
        usage = ""
        if total:
            usage = join_parts(
                [
                    f"{format_bytes(used)} / {format_bytes(total)}",
                    f"{percentage(used, total):.0f}%",
                    disk.get("filesystem") or "",
                ]
            )
        values.extend(facts(fact(f"Filesystem {name}", usage)))
    return values


def display_facts(snapshot):
    values = []
    for display in as_list(snapshot.result("Display", [])):
        output = as_dict(display.get("output"))
        scaled = as_dict(display.get("scaled"))
        display_values = []
        if output.get("width") and output.get("height"):
            resolution = f"{output['width']}×{output['height']}"
            if output.get("refreshRate"):
                resolution += f" @ {output['refreshRate']:.0f} Hz"
            display_values.append(resolution)
        if scaled and (scaled.get("width"), scaled.get("height")) != (
            output.get("width"),
            output.get("height"),
        ):
            display_values.append(f"scaled {scaled.get('width')}×{scaled.get('height')}")
        if display.get("hdrStatus") == "Supported":
            display_values.append("HDR")
        label = f"Display {display.get('name') or ''}".strip()
        values.extend(facts(fact(label, join_parts(display_values))))
    return values


def software_badges(snapshot):
    os_info = as_dict(snapshot.result("OS", {}))
    de = as_dict(snapshot.result("DE", {}))
    wm = as_dict(snapshot.result("WM", {}))
    badges = []
    os_name = os_info.get("prettyName") or os_info.get("name") or "Unknown system"
    platform = SoftwareBadge(
        "os",
        os_info.get("id") or os_name,
        os_name,
        (os_info.get("id") or "", os_name),
    )
    if snapshot.de_display != "unknown":
        badges.append(
            SoftwareBadge(
                "desktop",
                de.get("prettyName") or snapshot.de_display,
                snapshot.de_display,
                (de.get("processName") or "",),
            )
        )
    wm_name = wm.get("prettyName") or ""
    if wm_name:
        badges.append(
            SoftwareBadge(
                "wm",
                wm_name,
                wm_name,
                (wm.get("processName") or "",),
            )
        )
    if wm.get("protocolName"):
        badges.append(SoftwareBadge("session", wm["protocolName"], wm["protocolName"]))
    if snapshot.terminal_display != "unknown":
        badges.append(
            SoftwareBadge(
                "terminal",
                snapshot.terminal_display,
                snapshot.terminal_display,
            )
        )
    if snapshot.shell_display != "unknown":
        badges.append(
            SoftwareBadge(
                "shell",
                snapshot.shell_display,
                snapshot.shell_display,
            )
        )
    return platform, tuple(badges)
