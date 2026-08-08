from tools.utils.sysinfo.formatting import as_dict


def is_macos(snapshot):
    os_info = as_dict(snapshot.result("OS", {}))
    identity = " ".join(str(os_info.get(key) or "") for key in ("id", "name", "prettyName")).lower()
    return "macos" in identity or "darwin" in identity


def text_value(value):
    if isinstance(value, (list, tuple, set)):
        return ", ".join(str(item) for item in value if item not in (None, ""))
    return str(value) if value not in (None, "") else ""


def useful_device_name(value, fallback, rejected_prefixes=()):
    name = text_value(value).strip()
    lowered = name.lower()
    if not name or name.isdigit() or lowered in {"unknown", "n/a", "none", "null"}:
        return fallback
    if any(lowered.startswith(prefix.lower()) for prefix in rejected_prefixes):
        return fallback
    return name


def is_virtual_disk(disk):
    identity = " ".join(
        text_value(disk.get(key))
        for key in ("name", "kind", "interconnect", "volumeType", "mountFrom")
    ).lower()
    return any(
        marker in identity
        for marker in (
            "disk image",
            "virtual interface",
            "virtual",
            "loop device",
            "sparse image",
        )
    )


def is_actionable_filesystem(disk):
    if is_virtual_disk(disk) or disk.get("readOnly") is True:
        return False
    filesystem = str(disk.get("filesystem") or "").lower()
    if filesystem in {"devfs", "iso9660", "squashfs", "tmpfs", "udf"}:
        return False
    flags = text_value(disk.get("volumeType")).lower()
    if "read-only" in flags or "readonly" in flags:
        return False
    mountpoint = str(disk.get("mountpoint") or "")
    return not (mountpoint.startswith("/System/Volumes/") and mountpoint != "/System/Volumes/Data")
