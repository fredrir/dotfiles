import os
import shutil
import socket
import sys
from dataclasses import dataclass, field

from tools.core import blocks
from tools.core.paths import repo_root
from tools.core.process import capture

HARDWARE_KEYS = {
    "GPU": "gpu",
    "CPU": "cpu",
    "CPU_COOLER": "cpu_cooler",
    "MOTHERBOARD": "motherboard",
    "MEMORY": "memory",
    "STORAGE": "storage",
    "CASE": "case",
    "POWER_SUPPLY": "power_supply",
}

DEFAULT_HARDWARE = {"cpu_cooler": "not set", "case": "not set", "power_supply": "not set"}

ROLES = ("desktop", "laptop", "server")


@dataclass(frozen=True)
class Host:
    name: str
    hostnames: tuple[str, ...] = ()
    role: str = ""
    hardware: dict = field(default_factory=dict)

    def resolved_hardware(self):
        values = dict(DEFAULT_HARDWARE)
        values.update(self.hardware)
        return values


def hosts_file():
    override = os.environ.get("SYSINFO_CONFIG")
    if override:
        return override
    return str(repo_root() / "config/hosts.dotfile")


def describe_error(error):
    return blocks.describe(error, "config/hosts.dotfile", "host")


def load_hosts(path=None):
    # Whole-line comments only: this file carries a header, but '#' also appears
    # inside hardware values as part numbers and revisions. Stripping it there
    # silently truncated the value into every benchmark record that followed.
    entries = blocks.read(path or hosts_file(), comments=blocks.LINE)
    order = []
    fields = {}
    for entry in entries:
        if entry.opens:
            if entry.block not in fields:
                fields[entry.block] = {"hostnames": (), "role": "", "hardware": {}}
                order.append(entry.block)
            continue
        key, value = entry.split("=")
        collapsed = "".join(key.split()).upper()
        if collapsed == "HOSTNAMES":
            fields[entry.block]["hostnames"] = tuple(
                part.strip() for part in value.split(",") if part.strip()
            )
        elif collapsed == "ROLE":
            fields[entry.block]["role"] = value
        elif collapsed in HARDWARE_KEYS:
            fields[entry.block]["hardware"][HARDWARE_KEYS[collapsed]] = value
    return {name: Host(name, **fields[name]) for name in order}


def pin_path():
    config_home = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
    return os.path.join(config_home, "dotfile", "host")


def saved_host():
    try:
        with open(pin_path(), encoding="utf-8") as handle:
            return handle.read().strip()
    except OSError:
        return ""


def save_host(name):
    path = pin_path()
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(name + "\n")


def local_hostnames():
    names = []
    if sys.platform == "darwin" and shutil.which("scutil"):
        for key in ("LocalHostName", "ComputerName"):
            result = capture(["scutil", "--get", key])
            if result.returncode == 0 and result.stdout.strip():
                names.append(result.stdout.strip())
    raw = socket.gethostname()
    if raw:
        names.append(raw)
        names.append(raw.split(".", 1)[0])
    return tuple(dict.fromkeys(name for name in names if name))


def match_hostname(hosts, names=None):
    candidates = {name.lower() for name in (names if names is not None else local_hostnames())}
    for host in hosts.values():
        for alias in (host.name, *host.hostnames):
            if alias.lower() in candidates:
                return host.name
    return ""


def resolve(explicit="", hosts=None):
    for candidate in (explicit, os.environ.get("SYSINFO_HOST", ""), saved_host()):
        if candidate:
            return candidate
    return match_hostname(load_hosts() if hosts is None else hosts)


def render_host(host):
    lines = [f"{host.name} {{"]
    if host.hostnames:
        lines.append(f"  hostnames = {', '.join(host.hostnames)}")
    if host.role:
        lines.append(f"  role = {host.role}")
    labels = {value: key for key, value in HARDWARE_KEYS.items()}
    written = [key for key in HARDWARE_KEYS.values() if host.hardware.get(key)]
    if written:
        lines.append("")
        for key in written:
            lines.append(f"  {labels[key]} = {host.hardware[key]}")
    lines.append("}")
    return "\n".join(lines) + "\n"


def append_host(host, path=None):
    path = path or hosts_file()
    existing = ""
    if os.path.isfile(path):
        with open(path, encoding="utf-8") as handle:
            existing = handle.read()
    prefix = ""
    if existing and not existing.endswith("\n\n"):
        prefix = "\n" if existing.endswith("\n") else "\n\n"
    with open(path, "a", encoding="utf-8") as handle:
        handle.write(prefix + render_host(host))
    return path
