import csv
import json
import os
import shutil
import subprocess
import sys

from tools.core.console import die
from tools.core.paths import repo_root
from tools.core.process import capture
from tools.utils.sysinfo.formatting import as_list
from tools.utils.sysinfo.models import Snapshot

CONFIG_KEYS = {
    "GPU": "gpu",
    "CPU": "cpu",
    "CPU_COOLER": "cpu_cooler",
    "MOTHERBOARD": "motherboard",
    "MEMORY": "memory",
    "STORAGE": "storage",
    "CASE": "case",
    "POWER_SUPPLY": "power_supply",
}

TERMINALS = {
    "konsole": ("Konsole", "konsole"),
    "kitty": ("kitty", "kitty"),
    "alacritty": ("Alacritty", "alacritty"),
    "wezterm": ("WezTerm", "wezterm"),
    "wezterm-gui": ("WezTerm", "wezterm"),
    "ghostty": ("Ghostty", "ghostty"),
    "foot": ("foot", "foot"),
    "footclient": ("foot", "foot"),
    "tilix": ("Tilix", "tilix"),
    "xfce4-terminal": ("Xfce Terminal", "xfce4-terminal"),
}

SHELL_VERSION_PROBES = {
    "zsh": 'printf %s "$ZSH_VERSION"',
    "bash": 'printf %s "$BASH_VERSION"',
    "fish": 'printf %s "$version"',
}

BASE_MODULES = [
    "OS",
    "Kernel",
    "Shell",
    {"type": "CPU", "temp": True},
    {"type": "GPU", "temp": True, "driverSpecific": True},
    "Memory",
    "Swap",
    "Disk",
    "PhysicalMemory",
    {"type": "PhysicalDisk", "temp": True},
    "DE",
    "WM",
    "Terminal",
    "Board",
    "Battery",
    "PowerAdapter",
]

FULL_MODULES = [
    "Host",
    "Uptime",
    "Packages",
    "CPUCache",
    "CPUUsage",
    "OpenCL",
    "Vulkan",
    "TerminalFont",
    "Theme",
    "WMTheme",
    "Display",
    "BIOS",
    "Bootmgr",
    "InitSystem",
]


def load_hardware_config():
    values = {"cpu_cooler": "not set", "case": "not set", "power_supply": "not set"}
    config = os.environ.get("SYSINFO_CONFIG") or str(repo_root() / "hardware.dotfile")
    profile = os.environ.get("SYSINFO_HARDWARE")
    if not profile:
        profile = {"darwin": "macos", "win32": "windows"}.get(sys.platform, "desktop")
    try:
        with open(config, encoding="utf-8") as handle:
            lines = handle.read().splitlines()
    except OSError:
        return values
    active = False
    for raw in lines:
        line = raw.strip()
        if not active:
            active = line == f"{profile} {{"
            continue
        if line == "}":
            break
        if "=" not in line:
            continue
        key, _, value = line.partition("=")
        key = "".join(key.split())
        if key in CONFIG_KEYS:
            values[CONFIG_KEYS[key]] = value.strip()
    return values


def shell_info():
    shell_path = os.environ.get("SHELL", "")
    name = os.path.basename(shell_path)
    version = ""
    if shell_path and os.access(shell_path, os.X_OK) and name in SHELL_VERSION_PROBES:
        result = capture([shell_path, "-c", SHELL_VERSION_PROBES[name]])
        if result.returncode == 0:
            version = result.stdout
    return name, version


def process_field(pid, field):
    result = capture(["ps", "-o", f"{field}=", "-p", str(pid)])
    return result.stdout.strip()


def terminal_info():
    name = ""
    executable = ""
    ancestor = os.getppid()
    while ancestor > 1:
        process = process_field(ancestor, "comm")
        if process.startswith("gnome-terminal"):
            name, executable = "GNOME Terminal", "gnome-terminal"
        elif process in TERMINALS:
            name, executable = TERMINALS[process]
        if name:
            break
        parent = "".join(process_field(ancestor, "ppid").split())
        if not parent.isdigit():
            break
        ancestor = int(parent)
    version = ""
    if executable and shutil.which(executable):
        result = capture([executable, "--version"])
        first = result.stdout.split("\n", 1)[0]
        fields = first.split()
        if len(fields) > 1:
            version = fields[1]
    return name, version


def collect_fastfetch(full=False):
    if not shutil.which("fastfetch"):
        die("sysinfo", "fastfetch is required")
    modules = [*BASE_MODULES, *(FULL_MODULES if full else [])]
    result = capture(
        ["fastfetch", "-c", "-", "--format", "json"],
        input=json.dumps({"modules": modules}),
    )
    if result.returncode != 0:
        die("sysinfo", "fastfetch could not collect system information")
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        die("sysinfo", "fastfetch could not collect system information")


def numeric(value):
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def collect_nvidia():
    if not shutil.which("nvidia-smi"):
        return [], ""
    fields = (
        "index",
        "name",
        "memory.total",
        "memory.used",
        "utilization.gpu",
        "temperature.gpu",
        "power.draw",
        "power.limit",
        "clocks.current.graphics",
        "driver_version",
    )
    try:
        result = capture(
            [
                "nvidia-smi",
                f"--query-gpu={','.join(fields)}",
                "--format=csv,noheader,nounits",
            ],
            timeout=3,
        )
    except (OSError, subprocess.TimeoutExpired):
        return [], "NVIDIA live telemetry timed out"
    if result.returncode != 0:
        error_output = (result.stdout + result.stderr).lower()
        if "driver/library version mismatch" in error_output:
            return [], "NVIDIA kernel driver does not match the installed userspace library"
        return [], "NVIDIA live telemetry is unavailable"
    devices = []
    for row in csv.reader(result.stdout.splitlines(), skipinitialspace=True):
        if len(row) != len(fields):
            continue
        devices.append(
            {
                "index": int(row[0]) if row[0].isdigit() else None,
                "name": row[1],
                "memory_total_mib": numeric(row[2]),
                "memory_used_mib": numeric(row[3]),
                "utilization": numeric(row[4]),
                "temperature": numeric(row[5]),
                "power_draw": numeric(row[6]),
                "power_limit": numeric(row[7]),
                "clock_mhz": numeric(row[8]),
                "driver": row[9],
            }
        )
    return devices, ""


def index_modules(data):
    return {
        module["type"]: module["result"]
        for module in data
        if module.get("type") and module.get("result") is not None
    }


def versioned_name(preferred_name, preferred_version, fallback):
    name = preferred_name or fallback.get("prettyName") or "unknown"
    version = preferred_version or fallback.get("version") or ""
    return name + (f" {version}" if version else "")


def recognized_terminal(fallback):
    identity = " ".join(
        str(fallback.get(key) or "") for key in ("processName", "prettyName", "exeName", "exe")
    ).lower()
    known = (*TERMINALS, "gnome-terminal", "gnome terminal")
    return fallback if any(name in identity for name in known) else {}


def has_nvidia(modules):
    for gpu in as_list(modules.get("GPU")):
        identity = f"{gpu.get('vendor', '')} {gpu.get('name', '')}".lower()
        if "nvidia" in identity or "geforce" in identity:
            return True
    return False


def collect_snapshot(full=False):
    hardware = load_hardware_config()
    shell_name, shell_version = shell_info()
    terminal_name, terminal_version = terminal_info()
    modules = index_modules(collect_fastfetch(full=full))
    shell = modules.get("Shell") or {}
    terminal = recognized_terminal(modules.get("Terminal") or {})
    de = modules.get("DE") or {}
    wm = modules.get("WM") or {}

    de_display = de.get("prettyName") or "unknown"
    if de.get("version"):
        de_display += f" {de['version']}"

    wm_display = wm.get("prettyName") or "unknown"
    if wm.get("protocolName"):
        wm_display += f" ({wm['protocolName']})"

    nvidia = []
    probe_errors = []
    if has_nvidia(modules):
        nvidia, nvidia_error = collect_nvidia()
        if nvidia_error:
            probe_errors.append(nvidia_error)

    return Snapshot(
        hardware=hardware,
        modules=modules,
        shell_display=versioned_name(shell_name, shell_version, shell),
        terminal_display=versioned_name(terminal_name, terminal_version, terminal),
        de_display=de_display,
        wm_display=wm_display,
        nvidia=tuple(nvidia),
        probe_errors=tuple(probe_errors),
    )
