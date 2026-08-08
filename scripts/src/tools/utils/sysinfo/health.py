from collections import Counter

from tools.utils.sysinfo.devices import gpu_memory, named_gpu, nvidia_for
from tools.utils.sysinfo.formatting import (
    as_dict,
    as_list,
    configured_memory_bytes,
    format_bytes,
    format_temperature,
    memory_capacity,
    percentage,
)
from tools.utils.sysinfo.models import HealthIssue
from tools.utils.sysinfo.normalization import (
    is_actionable_filesystem,
    is_macos,
    is_virtual_disk,
    text_value,
)
from tools.utils.sysinfo.profiles import (
    CPU_PROFILES,
    DISK_PROFILES,
    GPU_PROFILES,
    profile_for,
)


def temperature_issue(name, temperature, maximum):
    if temperature is None or not maximum:
        return None
    if temperature >= maximum:
        return HealthIssue(
            "error",
            f"{name} temperature is above its limit",
            f"{format_temperature(temperature)} measured, {maximum}°C maximum",
            "Reduce load and verify cooling before continuing sustained work",
        )
    if temperature >= maximum * 0.85:
        return HealthIssue(
            "warning",
            f"{name} is running warm",
            f"{format_temperature(temperature)} measured, {maximum}°C maximum",
            "Check airflow and background load if the temperature continues rising",
        )
    return None


def probe_issue(message):
    if "kernel driver" in message.lower():
        return HealthIssue(
            "error",
            message,
            "NVIDIA telemetry cannot start while the versions differ",
            "Reboot to load the updated NVIDIA kernel module",
        )
    return HealthIssue("warning", message)


def health_issues(snapshot):
    issues = [probe_issue(message) for message in snapshot.probe_errors]
    memory = as_dict(snapshot.result("Memory", {}))
    detected = memory.get("total") or 0
    configured = configured_memory_bytes(snapshot.hardware.get("memory", ""))
    if configured and detected and detected < configured * 0.8:
        issues.append(
            HealthIssue(
                "warning",
                "Installed memory is not fully visible",
                f"Configured as {format_bytes(configured)}, detected as {memory_capacity(detected)}",
                "Check firmware memory training and reseat the DIMMs if this persists",
            )
        )
    memory_use = percentage(memory.get("used") or 0, detected)
    if memory_use >= 90 and not as_list(snapshot.result("Swap", [])) and not is_macos(snapshot):
        issues.append(
            HealthIssue(
                "error",
                "Memory pressure has no swap fallback",
                f"Memory is {memory_use:.0f}% used and swap is disabled",
                "Reduce memory use before starting another heavy workload",
            )
        )

    cpu = as_dict(snapshot.result("CPU", {}))
    cpu_name = cpu.get("cpu") or "CPU"
    cpu_profile = profile_for(cpu_name, CPU_PROFILES)
    issue = temperature_issue(cpu_name, cpu.get("temperature"), cpu_profile.max_temperature)
    if issue:
        issues.append(issue)

    for gpu in as_list(snapshot.result("GPU", [])):
        gpu_name = named_gpu(gpu)
        nvidia = nvidia_for(snapshot, gpu)
        gpu_profile = profile_for(gpu_name, GPU_PROFILES)
        temperature = nvidia.get("temperature")
        if temperature is None:
            temperature = gpu.get("temperature")
        issue = temperature_issue(gpu_name, temperature, gpu_profile.max_temperature)
        if issue:
            issues.append(issue)
        used, total = gpu_memory(gpu, nvidia)
        if total and percentage(used, total) >= 90:
            usage = percentage(used, total)
            issues.append(
                HealthIssue(
                    "warning",
                    f"{gpu_name} VRAM is nearly full",
                    f"{usage:.0f}% of VRAM is in use",
                    "Close GPU workloads or reduce their memory allocation",
                )
            )

    for disk in as_list(snapshot.result("Disk", [])):
        if not is_actionable_filesystem(disk):
            continue
        values = as_dict(disk.get("bytes"))
        disk_use = percentage(values.get("used") or 0, values.get("total") or 0)
        if disk_use >= 90:
            name = disk.get("mountpoint") or disk.get("name") or "Disk"
            issues.append(
                HealthIssue(
                    "warning",
                    f"{name} is nearly full",
                    f"{disk_use:.0f}% of the filesystem is used",
                    "Remove or relocate data before free space becomes critical",
                )
            )

    for disk in as_list(snapshot.result("PhysicalDisk", [])):
        if is_virtual_disk(disk):
            continue
        name = disk.get("name") or "Disk"
        disk_profile = profile_for(name, DISK_PROFILES)
        issue = temperature_issue(
            disk_profile.family or name,
            disk.get("temperature"),
            disk_profile.max_temperature,
        )
        if issue:
            issues.append(issue)

    for battery in as_list(snapshot.result("Battery", [])):
        capacity = battery.get("capacity")
        status = text_value(battery.get("status")).lower()
        charging = status.startswith("charging") or status in {
            "ac connected",
            "connected",
            "full",
            "fully charged",
        }
        if isinstance(capacity, (int, float)) and capacity <= 15 and not charging:
            severity = "error" if capacity <= 5 else "warning"
            issues.append(
                HealthIssue(
                    severity,
                    "Battery charge is low",
                    f"{capacity:.0f}% remaining",
                    "Connect external power",
                )
            )
    return tuple(issues)


def health_counts(issues):
    counts = Counter(issue.severity for issue in issues)
    return counts["error"], counts["warning"]


def health_summary(issues):
    errors, warnings = health_counts(issues)
    parts = []
    if errors:
        parts.append(f"{errors} error" + ("s" if errors != 1 else ""))
    if warnings:
        parts.append(f"{warnings} warning" + ("s" if warnings != 1 else ""))
    return "  ".join(parts)
