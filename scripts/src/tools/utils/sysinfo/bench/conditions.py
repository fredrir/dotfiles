import os
import shutil
import subprocess
import sys

from tools.core.process import capture as run
from tools.utils.sysinfo.bench.capture import detect_virtualized
from tools.utils.sysinfo.bench.limits import (
    LOAD_LIMIT,
    MEMORY_FILESYSTEMS,
    MIN_FREE_DISK,
    THROTTLE_MARGIN,
)
from tools.utils.sysinfo.formatting import as_dict, as_list
from tools.utils.sysinfo.profiles import CPU_PROFILES, profile_for

GOVERNOR_PATH = "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"

BATTERY_STATES = {"ac connected", "connected", "full", "fully charged"}


def cpu_governor():
    try:
        with open(GOVERNOR_PATH, encoding="utf-8") as handle:
            return handle.read().strip()
    except OSError:
        return ""


def on_battery(snapshot):
    batteries = as_list(snapshot.result("Battery", []))
    if not batteries:
        return False
    if as_list(snapshot.result("PowerAdapter", [])):
        return False
    for battery in batteries:
        status = str(battery.get("status") or "").lower()
        if status.startswith("charging") or status in BATTERY_STATES:
            return False
    return True


def load_average():
    try:
        return os.getloadavg()[0]
    except OSError:
        return None


def free_disk_ratio(path):
    try:
        stats = os.statvfs(path)
    except OSError:
        return None, None
    free = stats.f_bavail * stats.f_frsize
    total = stats.f_blocks * stats.f_frsize
    if not total:
        return free, None
    return free, free / total


def cpu_temperature(snapshot):
    cpu = as_dict(snapshot.result("CPU", {}))
    value = cpu.get("temperature")
    return value if isinstance(value, (int, float)) else None


def throttled(snapshot):
    temperature = cpu_temperature(snapshot)
    if temperature is None:
        return False
    cpu = as_dict(snapshot.result("CPU", {}))
    limit = profile_for(cpu.get("cpu") or "", CPU_PROFILES).max_temperature
    if not limit:
        return False
    return temperature >= limit - THROTTLE_MARGIN


THROTTLE_QUERIES = (
    "clocks_throttle_reasons.hw_thermal_slowdown",
    "clocks_throttle_reasons.sw_thermal_slowdown",
)


def nvidia_throttled():
    if not shutil.which("nvidia-smi"):
        return False
    # The timeout exists for a wedged driver, so the wedged driver must not then
    # surface as a traceback. capture_conditions runs before the job loop's
    # handler, and cool_down calls it again every 15s.
    try:
        result = run(
            ["nvidia-smi", f"--query-gpu={','.join(THROTTLE_QUERIES)}", "--format=csv,noheader"],
            timeout=3,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    if result.returncode != 0:
        return False
    for line in result.stdout.strip().splitlines():
        for field in line.split(","):
            if field.strip().lower() == "active":
                return True
    return False


def capture_conditions(snapshot, workdir):
    free_bytes, free_ratio = free_disk_ratio(workdir)
    memory = as_dict(snapshot.result("Memory", {}))
    total = memory.get("total") or 0
    used = memory.get("used") or 0
    return {
        "on_battery": on_battery(snapshot),
        "governor": cpu_governor(),
        "loadavg_1": load_average(),
        "cpu_count": os.cpu_count(),
        "idle_temp_c": cpu_temperature(snapshot),
        "free_ram_bytes": max(total - used, 0) or None,
        "free_disk_bytes": free_bytes,
        "free_disk_ratio": free_ratio,
        "throttled_at_start": throttled(snapshot) or nvidia_throttled(),
        "virtualized": detect_virtualized(),
        "platform": sys.platform,
    }


def gate_reasons(conditions, writes_disk=False):
    reasons = []
    if conditions.get("on_battery"):
        reasons.append("running on battery")
    load = conditions.get("loadavg_1")
    count = conditions.get("cpu_count") or 1
    if load is not None and load / count > LOAD_LIMIT:
        reasons.append(f"load average is {load:.2f} across {count} logical cores")
    if conditions.get("throttled_at_start"):
        reasons.append("the machine is already thermally throttled")
    ratio = conditions.get("free_disk_ratio")
    if writes_disk and ratio is not None and ratio < MIN_FREE_DISK:
        reasons.append(f"only {ratio * 100:.0f}% of the filesystem is free")
    fstype = as_dict(conditions.get("filesystem")).get("fstype", "")
    if writes_disk and fstype in MEMORY_FILESYSTEMS:
        reasons.append(f"the work directory is on {fstype}, which measures memory and not disk")
    return tuple(reasons)


def grade_for(reasons, metrics, failures=()):
    if not metrics:
        return "aborted"
    # A run where suites crashed is degraded, not clean. Grading it clean made it
    # baseline-eligible and reset the staleness clock while measuring nothing.
    if reasons or failures:
        return "noisy"
    return "clean"
