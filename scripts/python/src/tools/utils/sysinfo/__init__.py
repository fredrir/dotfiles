from tools.utils.sysinfo.branding import BrandProfile, illustration, resolve_brand
from tools.utils.sysinfo.cli import app, sysinfo
from tools.utils.sysinfo.collect import (
    collect_fastfetch,
    collect_nvidia,
    collect_snapshot,
    load_hardware_config,
    shell_info,
    terminal_info,
)
from tools.utils.sysinfo.devices import board_description, named_gpu
from tools.utils.sysinfo.formatting import capacity, memory_capacity
from tools.utils.sysinfo.health import health_issues
from tools.utils.sysinfo.hosts import Host, load_hosts, resolve
from tools.utils.sysinfo.models import (
    Component,
    Fact,
    HealthIssue,
    RenderOptions,
    Snapshot,
    SoftwareBadge,
    SystemView,
)
from tools.utils.sysinfo.plain import render_plain
from tools.utils.sysinfo.pretty import render_pretty
from tools.utils.sysinfo.view import build_view

__all__ = [
    "BrandProfile",
    "Component",
    "Fact",
    "HealthIssue",
    "Host",
    "RenderOptions",
    "Snapshot",
    "SoftwareBadge",
    "SystemView",
    "app",
    "board_description",
    "build_view",
    "capacity",
    "collect_fastfetch",
    "collect_nvidia",
    "collect_snapshot",
    "health_issues",
    "illustration",
    "load_hardware_config",
    "load_hosts",
    "memory_capacity",
    "named_gpu",
    "render_plain",
    "render_pretty",
    "resolve",
    "resolve_brand",
    "shell_info",
    "sysinfo",
    "terminal_info",
]
