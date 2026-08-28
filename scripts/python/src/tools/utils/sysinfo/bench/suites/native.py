"""Workloads measured by the bench-workloads Rust binary.

The other suites shell out to whatever the platform happens to package (7z,
sysbench, openssl), so their absolute numbers depend on how a distribution
built the tool. bench-workloads is compiled from this repository, pinned by
Cargo.lock, and dependency-free, so the same method runs on every host.
"""

import json
import os

from tools.core.paths import repo_root
from tools.core.process import capture as run
from tools.utils.sysinfo.bench.record import HIB, WORLD
from tools.utils.sysinfo.bench.suites import (
    Job,
    MeasurementError,
    Output,
    require,
    tool_path,
    version_of,
)

ITERATIONS = "800000000"
BUFFER_MIB = "256"
PASSES = "128"

RELEASE_BINARY = ("scripts", "rust", "target", "release", "bench-workloads")


def binary_path():
    override = os.environ.get("SYSINFO_BENCH_WORKLOADS")
    if override:
        return override if os.access(override, os.X_OK) else ""
    built = os.path.join(str(repo_root()), *RELEASE_BINARY)
    if os.access(built, os.X_OK):
        return built
    return tool_path("bench-workloads")


def measure(path, *args):
    result = require(run([path, *args], timeout=300), "bench-workloads")
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise MeasurementError("bench-workloads produced unreadable output") from error
    value = payload.get("value")
    if not isinstance(value, (int, float)) or value <= 0:
        raise MeasurementError("bench-workloads reported no value")
    return float(value)


def jobs(setting):
    path = binary_path()
    if not path:
        return []
    version = version_of(path, args=("--version",))
    return [
        Job(
            name="cpu.native",
            tool="bench-workloads",
            version=version,
            method="cpu.native/1.0.0",
            outputs=(
                Output("cpu.native_single", "Mops/s", HIB, WORLD),
                Output("cpu.native_multi", "Mops/s", HIB, WORLD),
            ),
            measure=lambda: {
                "cpu.native_single": measure(
                    path, "cpu", "--threads", "1", "--iterations", ITERATIONS
                ),
                "cpu.native_multi": measure(
                    path, "cpu", "--threads", "0", "--iterations", ITERATIONS
                ),
            },
            detail={"iterations": int(ITERATIONS)},
        ),
        Job(
            name="mem.native",
            tool="bench-workloads",
            version=version,
            method="mem.native/1.0.0",
            outputs=(
                Output("mem.native_read", "GiB/s", HIB, WORLD),
                Output("mem.native_write", "GiB/s", HIB, WORLD),
            ),
            measure=lambda: {
                "mem.native_read": measure(
                    path, "memory", "--op", "read", "--mib", BUFFER_MIB, "--passes", PASSES
                ),
                "mem.native_write": measure(
                    path, "memory", "--op", "write", "--mib", BUFFER_MIB, "--passes", PASSES
                ),
            },
            detail={"buffer_mib": int(BUFFER_MIB), "passes": int(PASSES), "threads": 1},
        ),
    ]
