import re

from tools.utils.sysinfo.branding import resolve_brand, strip_brand
from tools.utils.sysinfo.devices import cache_size, gpu_memory, named_gpu, nvidia_for, swap_totals
from tools.utils.sysinfo.facts import fact, facts
from tools.utils.sysinfo.formatting import (
    as_dict,
    as_list,
    average,
    capacity,
    format_bytes,
    format_frequency,
    format_temperature,
    join_parts,
    memory_capacity,
    memory_summary,
    percentage,
)
from tools.utils.sysinfo.models import Component
from tools.utils.sysinfo.normalization import (
    is_macos,
    is_virtual_disk,
    text_value,
    useful_device_name,
)
from tools.utils.sysinfo.profiles import (
    CPU_PROFILES,
    DISK_PROFILES,
    GPU_PROFILES,
    profile_for,
)


def cpu_component(snapshot):
    cpu = as_dict(snapshot.result("CPU", {}))
    name = cpu.get("cpu") or "Unknown CPU"
    vendor = cpu.get("vendor") or ""
    brand = resolve_brand("cpu", vendor, name)
    profile = profile_for(name, CPU_PROFILES)
    cores = as_dict(cpu.get("cores"))
    frequency = as_dict(cpu.get("frequency"))
    cache = as_dict(snapshot.result("CPUCache", {}))
    usage = average(as_list(snapshot.result("CPUUsage", [])))
    threads = ""
    if cores.get("physical") and cores.get("logical"):
        threads = f"{cores['physical']} cores / {cores['logical']} threads"
    l3 = cache_size(cache.get("l3"))
    family = profile.family or cpu.get("codeName") or ""
    return Component(
        kind="cpu",
        label="CPU",
        vendor=vendor or brand.name,
        model=strip_brand(name, brand),
        identifiers=(name, family),
        facts=facts(
            fact("Cores", threads),
            fact("Cache", f"{l3} L3" if l3 else ""),
            fact("Family", family),
            fact("Clock", format_frequency(frequency.get("max"))),
            fact("Power", f"{profile.tdp} W TDP" if profile.tdp else ""),
            fact("Temperature", format_temperature(cpu.get("temperature"))),
            fact("Load", f"{usage:.0f}%" if usage is not None else ""),
            fact("Process ISA", cpu.get("march") or ""),
            fact("Process", cpu.get("technology") or ""),
        ),
    )


def gpu_components(snapshot):
    components = []
    gpus = as_list(snapshot.result("GPU", []))
    discrete_present = any(gpu.get("type") == "Discrete" for gpu in gpus)
    for gpu in gpus:
        name = named_gpu(gpu)
        vendor = gpu.get("vendor") or ""
        brand = resolve_brand("gpu", vendor, name)
        profile = profile_for(name, GPU_PROFILES)
        nvidia = nvidia_for(snapshot, gpu)
        used, total = gpu_memory(gpu, nvidia)
        temperature = nvidia.get("temperature")
        if temperature is None:
            temperature = gpu.get("temperature")
        utilization = nvidia.get("utilization")
        if utilization is None:
            utilization = gpu.get("coreUsage")
        clock = nvidia.get("clock_mhz")
        if clock is None:
            clock = gpu.get("frequency")
        power_draw = nvidia.get("power_draw")
        power_limit = nvidia.get("power_limit")
        power = ""
        if power_draw is not None:
            power = f"{power_draw:.0f} W"
            if power_limit:
                power += f" / {power_limit:.0f} W"
        pcie = as_dict(gpu.get("pcieSpeed"))
        pcie_max = as_dict(pcie.get("max"))
        pcie_value = ""
        if pcie_max.get("gen") and pcie_max.get("lanes"):
            pcie_value = f"PCIe {pcie_max['gen']}.0 ×{pcie_max['lanes']}"
        driver = nvidia.get("driver") or gpu.get("driver") or ""
        integrated = gpu.get("type") == "Integrated"
        memory = profile.memory or (f"{format_bytes(total)} VRAM" if total else "")
        components.append(
            Component(
                kind="gpu",
                label="INTEGRATED GPU" if integrated else "GPU",
                vendor=vendor or brand.name,
                model=strip_brand(name, brand),
                identifiers=(name, gpu.get("driver") or ""),
                facts=facts(
                    fact("Architecture", profile.architecture),
                    fact("Memory", memory),
                    fact("Cores", profile.cores),
                    fact(
                        "VRAM use", f"{format_bytes(used)} / {format_bytes(total)}" if total else ""
                    ),
                    fact("Load", f"{utilization:.0f}%" if utilization is not None else ""),
                    fact("Clock", format_frequency(clock)),
                    fact("Temperature", format_temperature(temperature)),
                    fact("Power", power),
                    fact("Link", pcie_value),
                    fact("Driver", driver),
                ),
                compact=not integrated or not discrete_present,
            )
        )
    return components


def physical_memory_hints(snapshot):
    values = []
    for module in as_list(snapshot.result("PhysicalMemory", [])):
        for key in ("vendor", "manufacturer", "partNumber", "type"):
            if module.get(key):
                values.append(str(module[key]))
    return tuple(values)


def memory_component(snapshot):
    memory = as_dict(snapshot.result("Memory", {}))
    description = snapshot.hardware.get("memory", "")
    detected = memory.get("total") or 0
    hints = physical_memory_hints(snapshot)
    macos = is_macos(snapshot)
    brand = resolve_brand("memory", description, *hints, "Apple" if macos else "")
    kit_match = re.search(r"\b[A-Z0-9]{8,}\b", description)
    used = memory.get("used") or 0
    swap_used, swap_total = swap_totals(snapshot.result("Swap", []))
    module_count = 0 if macos else len(as_list(snapshot.result("PhysicalMemory", [])))
    swap = ""
    if swap_total:
        swap = f"{format_bytes(swap_used)} / {format_bytes(swap_total)}"
    elif not macos:
        swap = "Disabled"
    return Component(
        kind="memory",
        label="MEMORY",
        vendor=brand.name,
        model=memory_summary(description, detected),
        identifiers=(description, *hints),
        facts=facts(
            fact("Kit", kit_match.group(0) if kit_match else ""),
            fact("Modules", str(module_count) if module_count else ""),
            fact("Detected", memory_capacity(detected)),
            fact("Usage", f"{format_bytes(used)} / {format_bytes(detected)}" if detected else ""),
            fact("Load", f"{percentage(used, detected):.0f}%" if detected else ""),
            fact("Swap", swap),
        ),
    )


def motherboard_component(snapshot):
    board = as_dict(snapshot.result("Board", {}))
    configured = snapshot.hardware.get("motherboard", "")
    vendor = board.get("vendor") or ""
    model = configured or board.get("name") or "Unknown motherboard"
    brand = resolve_brand("motherboard", vendor, model)
    return Component(
        kind="motherboard",
        label="MOTHERBOARD",
        vendor=vendor or brand.name,
        model=strip_brand(model, brand),
        identifiers=(vendor, model),
        facts=facts(
            fact("Revision", board.get("version") or ""),
        ),
    )


def disk_components(snapshot):
    components = []
    for disk in as_list(snapshot.result("PhysicalDisk", [])):
        if is_virtual_disk(disk):
            continue
        name = disk.get("name") or "Unknown disk"
        profile = profile_for(name, DISK_PROFILES)
        brand = resolve_brand("storage", name)
        model = strip_brand(profile.family, brand) if profile.family else ""
        model = model or strip_brand(name.removeprefix("ATA "), brand)
        components.append(
            Component(
                kind="storage",
                label="STORAGE",
                vendor=brand.name,
                model=join_parts([model, capacity(disk.get("size")), disk.get("kind")]),
                identifiers=(name, profile.family),
                facts=facts(
                    fact("Device", name.removeprefix("ATA ")),
                    fact("Capacity", capacity(disk.get("size"))),
                    fact("Interface", disk.get("interconnect") or ""),
                    fact("Type", disk.get("kind") or ""),
                    fact("Temperature", format_temperature(disk.get("temperature"))),
                ),
            )
        )
    if components:
        return components
    total = sum(
        as_dict(disk.get("bytes")).get("total") or 0
        for disk in as_list(snapshot.result("Disk", []))
    )
    if not total:
        return []
    return [
        Component(
            kind="storage",
            label="STORAGE",
            vendor="",
            model=capacity(total),
        )
    ]


def configured_components(snapshot):
    values = (
        ("cooling", "CPU COOLING", snapshot.hardware.get("cpu_cooler", "")),
        ("case", "CHASSIS", snapshot.hardware.get("case", "")),
        ("power", "POWER SUPPLY", snapshot.hardware.get("power_supply", "")),
    )
    components = []
    for kind, label, description in values:
        if not description or description == "not set":
            continue
        brand = resolve_brand(kind, description)
        components.append(
            Component(
                kind=kind,
                label=label,
                vendor=brand.name,
                model=strip_brand(description, brand),
                identifiers=(description,),
                compact=False,
            )
        )
    return components


def portable_power_components(snapshot):
    components = []
    macos = is_macos(snapshot)
    for battery in as_list(snapshot.result("Battery", [])):
        raw_name = battery.get("modelName") or battery.get("name")
        name = useful_device_name(raw_name, "Internal battery", ("bq", "smc"))
        vendor = battery.get("manufacturer") or ("Apple" if macos else "")
        components.append(
            Component(
                kind="power",
                label="BATTERY",
                vendor=vendor,
                model=name,
                art_kind="battery",
                identifiers=(vendor, name),
                facts=facts(
                    fact(
                        "Charge",
                        f"{battery.get('capacity'):.0f}%"
                        if isinstance(battery.get("capacity"), (int, float))
                        else "",
                    ),
                    fact("Status", text_value(battery.get("status"))),
                    fact("Temperature", format_temperature(battery.get("temperature"))),
                    fact(
                        "Cycles",
                        str(battery["cycleCount"]) if battery.get("cycleCount") is not None else "",
                    ),
                ),
            )
        )
    for adapter in as_list(snapshot.result("PowerAdapter", [])):
        watts = adapter.get("watts")
        fallback = f"{watts} W" if watts else "Connected"
        name = useful_device_name(adapter.get("modelName") or adapter.get("name"), fallback)
        vendor = adapter.get("manufacturer") or ("Apple" if macos else "")
        output = f"{watts} W" if watts and name != f"{watts} W" else ""
        components.append(
            Component(
                kind="power",
                label="POWER ADAPTER",
                vendor=vendor,
                model=name,
                art_kind="adapter",
                identifiers=(vendor, name),
                facts=facts(
                    fact("Output", output),
                ),
                compact=False,
            )
        )
    return components


def hardware_components(snapshot):
    return (
        cpu_component(snapshot),
        *gpu_components(snapshot),
        memory_component(snapshot),
        motherboard_component(snapshot),
        *disk_components(snapshot),
        *configured_components(snapshot),
        *portable_power_components(snapshot),
    )
