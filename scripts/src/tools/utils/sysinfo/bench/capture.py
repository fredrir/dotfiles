import os
import shutil
import sys

from tools.core.paths import repo_root
from tools.core.process import capture as run
from tools.utils.sysinfo.devices import gpu_memory, named_gpu, nvidia_for
from tools.utils.sysinfo.formatting import as_dict, as_list
from tools.utils.sysinfo.normalization import is_virtual_disk


def detect_virtualized():
    if shutil.which("systemd-detect-virt"):
        result = run(["systemd-detect-virt"])
        detected = result.stdout.strip()
        if detected and detected != "none":
            return True
    try:
        with open("/proc/cpuinfo", encoding="utf-8") as handle:
            return " hypervisor" in handle.read()
    except OSError:
        return False


def describe_snapshot(snapshot):
    cpu = as_dict(snapshot.result("CPU", {}))
    cores = as_dict(cpu.get("cores"))
    frequency = as_dict(cpu.get("frequency"))
    memory = as_dict(snapshot.result("Memory", {}))
    board = as_dict(snapshot.result("Board", {}))
    gpus = []
    for gpu in as_list(snapshot.result("GPU", [])):
        _used, total = gpu_memory(gpu, nvidia_for(snapshot, gpu))
        gpus.append(
            {
                "name": named_gpu(gpu),
                "vendor": gpu.get("vendor") or "",
                "type": gpu.get("type") or "",
                "memory_total": total or None,
                "driver": gpu.get("driver") or "",
            }
        )
    disks = []
    for disk in as_list(snapshot.result("PhysicalDisk", [])):
        if is_virtual_disk(disk):
            continue
        disks.append(
            {
                "name": (disk.get("name") or "").removeprefix("ATA "),
                "size": disk.get("size") or None,
                "kind": disk.get("kind") or "",
                "interconnect": disk.get("interconnect") or "",
            }
        )
    return {
        "cpu": {
            "model": cpu.get("cpu") or "",
            "vendor": cpu.get("vendor") or "",
            "cores_physical": cores.get("physical"),
            "cores_logical": cores.get("logical"),
            "frequency_max": frequency.get("max"),
            "march": cpu.get("march") or "",
        },
        "gpu": gpus,
        "memory": {
            "total": memory.get("total"),
            "modules": len(as_list(snapshot.result("PhysicalMemory", []))),
        },
        "board": {"vendor": board.get("vendor") or "", "name": board.get("name") or ""},
        "disks": disks,
        "configured": dict(snapshot.hardware),
        "virtualized": detect_virtualized(),
    }


def describe_install(snapshot):
    operating = as_dict(snapshot.result("OS", {}))
    kernel = as_dict(snapshot.result("Kernel", {}))
    driver = ""
    for device in snapshot.nvidia:
        if device.get("driver"):
            driver = device["driver"]
            break
    if not driver:
        for gpu in as_list(snapshot.result("GPU", [])):
            if gpu.get("driver"):
                driver = gpu["driver"]
                break
    return {
        "os": operating.get("id") or operating.get("name") or sys.platform,
        "name": operating.get("prettyName") or operating.get("name") or "",
        "version": operating.get("versionID") or operating.get("version") or "",
        "kernel": kernel.get("release") or "",
        "arch": kernel.get("architecture") or os.uname().machine,
        "driver": driver,
    }


def dotfiles_sha():
    root = str(repo_root())
    result = run(["git", "-C", root, "rev-parse", "--short", "HEAD"])
    if result.returncode != 0:
        return ""
    sha = result.stdout.strip()
    dirty = run(["git", "-C", root, "status", "--porcelain", "--untracked-files=no"])
    if dirty.returncode == 0 and dirty.stdout.strip():
        return f"{sha}-dirty"
    return sha
