import os
import re
import sys

from tools.core.process import capture as run
from tools.utils.sysinfo.bench.record import HIB, HOST
from tools.utils.sysinfo.bench.suites import (
    Job,
    MeasurementError,
    Output,
    require,
    tool_path,
    version_of,
)

SCORE = re.compile(r"Score:\s*(\d+)")

SCENES = ("build:duration=2", "texture:duration=2", "shading:duration=2")

METAL_SOURCE = os.path.join(os.path.dirname(__file__), "metal", "gpu_bench.swift")


def session_environment():
    values = dict(os.environ)
    if values.get("WAYLAND_DISPLAY") or values.get("DISPLAY"):
        return values
    runtime = values.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"
    result = run(
        ["systemctl", "--user", "show-environment"],
        env={**values, "XDG_RUNTIME_DIR": runtime},
    )
    if result.returncode != 0:
        return values
    for line in result.stdout.splitlines():
        key, _, value = line.partition("=")
        if key in ("WAYLAND_DISPLAY", "DISPLAY", "XDG_RUNTIME_DIR") and value:
            values[key] = value
    values.setdefault("XDG_RUNTIME_DIR", runtime)
    return values


def has_display(values):
    return bool(values.get("WAYLAND_DISPLAY") or values.get("DISPLAY"))


def glmark_path(values):
    if values.get("WAYLAND_DISPLAY"):
        return tool_path("glmark2-wayland", "glmark2-es2-wayland")
    if values.get("DISPLAY"):
        return tool_path("glmark2", "glmark2-es2")
    return ""


def cache_dir():
    base = os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache")
    path = os.path.join(base, "dotfile", "bench")
    os.makedirs(path, exist_ok=True)
    return path


def marked_score(path, arguments, tool, values=None):
    result = require(run([path, *arguments], timeout=600, env=values), tool)
    match = SCORE.search(result.stdout)
    if not match:
        raise MeasurementError(f"{tool} reported no score")
    return float(match.group(1))


def metal_binary():
    if sys.platform != "darwin" or not os.path.isfile(METAL_SOURCE):
        return ""
    binary = os.path.join(cache_dir(), "gpu_bench")
    if os.path.isfile(binary) and os.path.getmtime(binary) >= os.path.getmtime(METAL_SOURCE):
        return binary
    compiler = tool_path("swiftc")
    if not compiler:
        return ""
    result = run([compiler, "-O", METAL_SOURCE, "-o", binary], timeout=600)
    if result.returncode != 0:
        return ""
    return binary


def metal_throughput(binary):
    result = require(run([binary], timeout=300), "gpu_bench")
    values = re.findall(r"([\d.]+)", result.stdout)
    if not values:
        raise MeasurementError("gpu_bench reported no throughput")
    return float(values[-1])


def jobs(setting):
    found = []
    values = session_environment()
    display = has_display(values)
    glmark = glmark_path(values)
    if glmark:
        arguments = ["--off-screen"]
        for scene in SCENES:
            arguments.extend(["-b", scene])
        found.append(
            Job(
                name="gpu.graphics",
                tool=os.path.basename(glmark),
                version=version_of(glmark, args=("--version",), pattern=r"(\d[\d.]*)"),
                method="gpu.graphics/1.0.0",
                outputs=(Output("gpu.graphics", "score", HIB, HOST),),
                measure=lambda: {
                    "gpu.graphics": marked_score(glmark, arguments, "glmark2", values)
                },
                repeat=False,
                detail={"scenes": list(SCENES), "mode": "off-screen"},
            )
        )
    vkmark = tool_path("vkmark")
    if vkmark and display and setting.tier in ("standard", "heavy"):
        found.append(
            Job(
                name="gpu.vulkan",
                tool="vkmark",
                version=version_of(vkmark, args=("--version",), pattern=r"(\d[\d.]*)"),
                method="gpu.vulkan/1.0.0",
                outputs=(Output("gpu.vulkan", "score", HIB, HOST),),
                measure=lambda: {"gpu.vulkan": marked_score(vkmark, [], "vkmark", values)},
                repeat=False,
            )
        )
    binary = metal_binary()
    if binary:
        found.append(
            Job(
                name="gpu.compute",
                tool="metal",
                version="1.0.0",
                method="gpu.compute/1.0.0",
                outputs=(Output("gpu.compute", "GFLOPS", HIB, HOST),),
                measure=lambda: {"gpu.compute": metal_throughput(binary)},
                repeat=False,
                detail={"kernel": "fma", "api": "Metal"},
            )
        )
    return found
