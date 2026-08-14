import math
import re

GIB = 1024**3
MIB = 1024**2


def as_dict(value):
    return value if isinstance(value, dict) else {}


def as_list(value):
    return value if isinstance(value, list) else []


def compact_number(value, digits=1):
    if value is None:
        return ""
    rounded = round(value, digits)
    if rounded == int(rounded):
        return str(int(rounded))
    return f"{rounded:.{digits}f}"


def tenth(value):
    rounded = math.floor(value + 0.5)
    if rounded % 10 == 0:
        return str(rounded // 10)
    return f"{rounded // 10}.{rounded % 10}"


def capacity(size):
    size = size or 0
    if size >= 1000000000000:
        return f"{tenth(size / 100000000000)} TB"
    if size >= 1000000000:
        return f"{tenth(size / 100000000)} GB"
    if size > 0:
        return f"{tenth(size / 100000)} MB"
    return "unknown"


def memory_capacity(size):
    size = size or 0
    if size > 0:
        return f"{math.ceil(size / GIB / 8) * 8} GB"
    return "unknown"


def format_bytes(value):
    value = value or 0
    for divisor, suffix in (
        (1024**4, "TB"),
        (GIB, "GB"),
        (MIB, "MB"),
        (1024, "KB"),
    ):
        if value >= divisor:
            return f"{compact_number(value / divisor)} {suffix}"
    return f"{int(value)} B"


def format_frequency(value):
    if not value:
        return ""
    return f"{compact_number(value / 1000, 2)} GHz"


def format_temperature(value):
    if value is None:
        return ""
    return f"{compact_number(value)}°C"


def format_duration(milliseconds):
    seconds = int((milliseconds or 0) / 1000)
    days, seconds = divmod(seconds, 86400)
    hours, seconds = divmod(seconds, 3600)
    minutes, _seconds = divmod(seconds, 60)
    parts = []
    if days:
        parts.append(f"{days}d")
    if hours:
        parts.append(f"{hours}h")
    if minutes or not parts:
        parts.append(f"{minutes}m")
    return " ".join(parts)


def join_parts(parts, separator="  "):
    return separator.join(str(part) for part in parts if part not in (None, ""))


def percentage(used, total):
    if not total:
        return 0.0
    return max(0.0, min(100.0, used / total * 100))


def configured_memory_bytes(description):
    matches = re.findall(
        r"(?<![A-Z0-9])([0-9]+(?:\.[0-9]+)?)\s*(TB|GB|MB)\b",
        description.upper(),
    )
    if not matches:
        return 0
    amount, unit = matches[0]
    multiplier = {"TB": 1024**4, "GB": GIB, "MB": MIB}[unit]
    return float(amount) * multiplier


def memory_summary(description, detected):
    values = []
    capacity_match = re.search(
        r"\b\d+(?:\.\d+)?\s*(?:TB|GB|MB)\b",
        description,
        re.IGNORECASE,
    )
    speed_match = re.search(r"\b(?:LP)?DDR\d(?:-\d+)?\b", description, re.IGNORECASE)
    timing_match = re.search(r"\bCL\d+\b", description, re.IGNORECASE)
    for match in (capacity_match, speed_match, timing_match):
        if match:
            value = match.group(0).upper()
            if value not in values:
                values.append(value)
    if not values:
        values.append(memory_capacity(detected))
    return join_parts(values)


def average(values):
    numeric = [value for value in values if isinstance(value, (int, float))]
    return sum(numeric) / len(numeric) if numeric else None
