from tools.utils.sysinfo.formatting import MIB, as_dict, as_list, capacity, format_bytes
from tools.utils.sysinfo.models import Snapshot


def named_gpu(gpu):
    vendor = gpu.get("vendor") or ""
    name = gpu.get("name") or ""
    if vendor and not name.lower().startswith(vendor.lower()):
        return f"{vendor} {name}"
    return name or "unknown"


def board_description(board):
    vendor = board.get("vendor") or ""
    if vendor == "ASUSTeK COMPUTER INC.":
        vendor = "ASUS"
    parts = [part for part in (vendor, board.get("name") or "") if part]
    return " ".join(parts)


def disk_description(disk):
    name = disk.get("name") or "Unknown disk"
    values = [name, capacity(disk.get("size"))]
    if disk.get("interconnect"):
        values.append(disk["interconnect"])
    return " ".join(values)


def cache_size(cache):
    total = sum((entry.get("size") or 0) * (entry.get("num") or 1) for entry in as_list(cache))
    return format_bytes(total) if total else ""


def nvidia_for(snapshot: Snapshot, gpu):
    index = gpu.get("index")
    name = named_gpu(gpu).lower()
    for device in snapshot.nvidia:
        if index is not None and device.get("index") == index:
            return device
        device_name = (device.get("name") or "").lower()
        if device_name and (device_name in name or name in device_name):
            return device
    return {}


def gpu_memory(gpu, nvidia):
    if nvidia.get("memory_total_mib") is not None:
        return (
            (nvidia.get("memory_used_mib") or 0) * MIB,
            nvidia["memory_total_mib"] * MIB,
        )
    memory = as_dict(gpu.get("memory"))
    dedicated = as_dict(memory.get("dedicated"))
    return dedicated.get("used") or 0, dedicated.get("total") or 0


def swap_totals(swaps):
    total = 0
    used = 0
    for swap in as_list(swaps):
        values = as_dict(swap.get("bytes"))
        total += values.get("total") or 0
        used += values.get("used") or 0
    return used, total
