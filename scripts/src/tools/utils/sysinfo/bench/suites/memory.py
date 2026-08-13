import re

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

TOTAL_SIZE = "64G"
BLOCK_SIZE = "1M"
RANDOM_BLOCK = "64"
SECONDS = "2"

TRANSFER = re.compile(r"\(([\d.]+)\s*MiB/sec\)")


def sysbench_memory(path, operation, block, mode):
    result = require(
        run(
            [
                path,
                "memory",
                f"--memory-block-size={block}",
                f"--memory-total-size={TOTAL_SIZE}",
                f"--memory-oper={operation}",
                f"--memory-access-mode={mode}",
                f"--time={SECONDS}",
                "run",
            ],
            timeout=120,
        ),
        "sysbench",
    )
    match = TRANSFER.search(result.stdout)
    if not match:
        raise MeasurementError(f"sysbench reported no {mode} {operation} throughput")
    return float(match.group(1))


def jobs(setting):
    path = tool_path("sysbench")
    if not path:
        return []
    version = version_of(path, args=("--version",), pattern=r"(\d[\d.]*)")
    return [
        Job(
            name="mem.bandwidth",
            tool="sysbench",
            version=version,
            method="mem.bandwidth/1.0.0",
            outputs=(
                Output("mem.write", "MiB/s", HIB, WORLD),
                Output("mem.read", "MiB/s", HIB, WORLD),
            ),
            measure=lambda: {
                "mem.write": sysbench_memory(path, "write", BLOCK_SIZE, "seq"),
                "mem.read": sysbench_memory(path, "read", BLOCK_SIZE, "seq"),
            },
            detail={"block": BLOCK_SIZE, "seconds": SECONDS, "mode": "seq"},
        ),
        Job(
            name="mem.random",
            tool="sysbench",
            version=version,
            method="mem.random/1.0.0",
            outputs=(Output("mem.random", "MiB/s", HIB, WORLD),),
            measure=lambda: {
                "mem.random": sysbench_memory(path, "read", RANDOM_BLOCK, "rnd"),
            },
            detail={"block": RANDOM_BLOCK, "seconds": SECONDS, "mode": "rnd"},
        ),
    ]
