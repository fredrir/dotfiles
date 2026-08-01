import json
import math
import os
import shutil

import typer

from tools.core.console import die, out
from tools.core.paths import repo_root
from tools.core.process import capture

app = typer.Typer(add_completion=False)

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

FASTFETCH_STRUCTURE = "OS:Kernel:Shell:CPU:GPU:Memory:Disk:DE:WM:Terminal:Board:PhysicalDisk"


def load_hardware_config():
    values = {"cpu_cooler": "not set", "case": "not set", "power_supply": "not set"}
    config = os.environ.get("SYSINFO_CONFIG") or str(repo_root() / "hardware.dotfile")
    profile = os.environ.get("SYSINFO_HARDWARE", "desktop")
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


def nvidia_vram():
    if not shutil.which("nvidia-smi"):
        return ""
    result = capture(["nvidia-smi", "--query-gpu=memory.total", "--format=csv,noheader,nounits"])
    first = result.stdout.split("\n", 1)[0]
    mib = "".join(first.split())
    if not mib.isdigit():
        return ""
    return f"{(int(mib) + 512) // 1024} GB"


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
        return f"{math.ceil(size / 1073741824 / 8) * 8} GB"
    return "unknown"


def named_gpu(gpu):
    vendor = gpu.get("vendor") or ""
    name = gpu.get("name") or ""
    if vendor and not name.startswith(vendor):
        return f"{vendor} {name}"
    return name or "unknown"


def present(value, fallback):
    return value if value else fallback


def module_result(data, kind):
    for module in data:
        if module.get("type") == kind and module.get("result") is not None:
            return module["result"]
    return {}


def as_list(value):
    return value if isinstance(value, list) else []


def collect_fastfetch():
    if not shutil.which("fastfetch"):
        die("sysinfo", "fastfetch is required")
    result = capture(
        ["fastfetch", "-c", "none", "--format", "json", "--structure", FASTFETCH_STRUCTURE]
    )
    if result.returncode != 0:
        die("sysinfo", "fastfetch could not collect system information")
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        die("sysinfo", "fastfetch could not collect system information")


def disk_description(disk):
    text = f"{disk.get('name')} {capacity(disk.get('size'))}"
    interconnect = disk.get("interconnect") or ""
    if interconnect:
        text += f" {interconnect}"
    return text


def board_description(board):
    vendor = board.get("vendor") or ""
    if vendor == "ASUSTeK COMPUTER INC.":
        vendor = "ASUS"
    parts = [part for part in (vendor, board.get("name") or "") if part]
    return " ".join(parts)


@app.command(help="Summarise the environment and hardware of this machine.")
def sysinfo():
    hardware = load_hardware_config()
    gpu_vram = nvidia_vram()
    shell_name, shell_version = shell_info()
    terminal_name, terminal_version = terminal_info()
    data = collect_fastfetch()

    os_info = module_result(data, "OS")
    kernel = module_result(data, "Kernel")
    shell = module_result(data, "Shell")
    cpu = module_result(data, "CPU")
    all_gpus = as_list(module_result(data, "GPU"))
    memory = module_result(data, "Memory")
    logical_disks = as_list(module_result(data, "Disk"))
    de = module_result(data, "DE")
    wm = module_result(data, "WM")
    terminal = module_result(data, "Terminal")
    board = module_result(data, "Board")
    physical_disks = as_list(module_result(data, "PhysicalDisk"))

    discrete = [gpu for gpu in all_gpus if (gpu.get("type") or "") == "Discrete"]
    gpus = discrete if discrete else all_gpus
    gpu_name = " + ".join(named_gpu(gpu) for gpu in gpus)
    disks = " + ".join(disk_description(disk) for disk in physical_disks)
    physical_size = sum(disk.get("size") or 0 for disk in physical_disks)
    if physical_size > 0:
        disk_capacity = capacity(physical_size)
    else:
        disk_capacity = capacity(
            sum((disk.get("bytes") or {}).get("total") or 0 for disk in logical_disks)
        )
    memory_total = memory_capacity(memory.get("total") or 0)

    gpu_display = present(gpu_name, "unknown")
    hardware_gpu = present(
        hardware.get("gpu", ""),
        gpu_display + (f" {gpu_vram}" if gpu_vram else ""),
    )
    hardware_cpu = present(hardware.get("cpu", ""), cpu.get("cpu") or "unknown")
    motherboard = present(
        hardware.get("motherboard", ""), present(board_description(board), "unknown")
    )
    hardware_memory = present(hardware.get("memory", ""), memory_total)
    storage = present(hardware.get("storage", ""), present(disks, "unknown"))

    shell_display = present(shell_name, shell.get("prettyName") or "unknown")
    if shell_version:
        shell_display += f" {shell_version}"
    elif shell.get("version") or "":
        shell_display += f" {shell['version']}"

    de_display = de.get("prettyName") or "unknown"
    if de.get("version") or "":
        de_display += f" {de['version']}"

    wm_display = wm.get("prettyName") or "unknown"
    if wm.get("protocolName") or "":
        wm_display += f" ({wm['protocolName']})"

    terminal_display = present(terminal_name, terminal.get("prettyName") or "unknown")
    if terminal_version:
        terminal_display += f" {terminal_version}"
    elif terminal.get("version") or "":
        terminal_display += f" {terminal['version']}"

    out(
        "Environment: "
        f"OS={os_info.get('prettyName') or 'unknown'}, "
        f"Kernel={kernel.get('release') or 'unknown'}, "
        f"Shell={shell_display}, "
        f"CPU={cpu.get('cpu') or 'unknown'}, "
        f"GPU={gpu_display}, "
        f"Memory={memory_total}, "
        f"Disk={disk_capacity}, "
        f"DE={de_display}, "
        f"WM={wm_display}, "
        f"Terminal={terminal_display}"
    )
    out(
        "Hardware: "
        f"GPU={hardware_gpu}, "
        f"CPU={hardware_cpu}, "
        f"CPU cooler={hardware['cpu_cooler']}, "
        f"Motherboard={motherboard}, "
        f"Memory={hardware_memory}, "
        f"Storage={storage}, "
        f"Case={hardware['case']}, "
        f"Power supply={hardware['power_supply']}"
    )
