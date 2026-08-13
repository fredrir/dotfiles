import os
import re

from tools.core.process import capture as run
from tools.utils.sysinfo.bench.record import HIB, HOST, WORLD
from tools.utils.sysinfo.bench.suites import (
    Job,
    MeasurementError,
    Output,
    require,
    tool_path,
    version_of,
)

TOTAL_SIZE = "64G"
SECONDS = "2"

# sysbench sizes its working set from --memory-block-size; --memory-total-size
# only decides how many times that buffer is traversed. A 1 MiB buffer therefore
# lives in cache, which is worth measuring but is not memory bandwidth.
CACHE_BLOCK = "1M"

# Ten times the largest L3 in current desktop parts, because a buffer only a
# couple of times the cache still reads partly from it: on a 96 MiB X3D part a
# 256 MiB block measures 15% faster than a 1 GiB one.
DRAM_BLOCK = "1G"
SMALL_DRAM_BLOCK = "256M"
SMALL_RAM = 8 * 1024**3

# Single threaded, deliberately. Threading looks like the way to saturate a
# memory controller, and on Apple Silicon it does -- but on a 9800X3D sysbench
# then reports 241% of what DDR5-6000 can physically carry, because a buffer
# that is never written maps every page to the shared zero page and all threads
# read it out of cache. A metric that can exceed the bus is the defect this
# split exists to remove, so mem.* measures what one core can pull: always a
# floor on the machine, never a physical impossibility.
THREADS = "1"

TRANSFER = re.compile(r"\(([\d.]+)\s*MiB/sec\)")

UNITS = {"k": 1024, "m": 1024**2, "g": 1024**3}


def parse_size(value):
    suffix = value[-1].lower()
    if suffix in UNITS:
        return int(float(value[:-1]) * UNITS[suffix])
    return int(value)


def physical_memory():
    try:
        return os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES")
    except (AttributeError, OSError, ValueError):
        return 0


def dram_block():
    total = physical_memory()
    if total and total < SMALL_RAM:
        return SMALL_DRAM_BLOCK
    return DRAM_BLOCK


def sysbench_memory(path, operation, block, mode, threads=THREADS):
    result = require(
        run(
            [
                path,
                "memory",
                f"--memory-block-size={block}",
                f"--memory-total-size={TOTAL_SIZE}",
                f"--memory-oper={operation}",
                f"--memory-access-mode={mode}",
                f"--threads={threads}",
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
    block = dram_block()
    return [
        Job(
            name="mem.bandwidth",
            tool="sysbench",
            version=version,
            method="mem.bandwidth/3.0.0",
            outputs=(
                Output("mem.write", "MiB/s", HIB, WORLD),
                Output("mem.read", "MiB/s", HIB, WORLD),
            ),
            measure=lambda: {
                "mem.write": sysbench_memory(path, "write", block, "seq"),
                "mem.read": sysbench_memory(path, "read", block, "seq"),
            },
            detail={
                "block": block,
                "seconds": SECONDS,
                "mode": "seq",
                "threads": int(THREADS),
                "working_set": parse_size(block),
            },
        ),
        Job(
            name="mem.random",
            tool="sysbench",
            version=version,
            method="mem.random/3.0.0",
            outputs=(Output("mem.random", "MiB/s", HIB, WORLD),),
            measure=lambda: {
                "mem.random": sysbench_memory(path, "read", block, "rnd"),
            },
            detail={
                "block": block,
                "seconds": SECONDS,
                "mode": "rnd",
                "threads": int(THREADS),
                "working_set": parse_size(block),
            },
        ),
        Job(
            name="cache.bandwidth",
            tool="sysbench",
            version=version,
            # Scoped to one machine: what a 1 MiB buffer costs depends on where
            # it lands in a particular cache hierarchy, so the number says
            # nothing when held against a different design.
            method="cache.bandwidth/1.0.0",
            outputs=(
                Output("cache.write", "MiB/s", HIB, HOST),
                Output("cache.read", "MiB/s", HIB, HOST),
            ),
            measure=lambda: {
                "cache.write": sysbench_memory(path, "write", CACHE_BLOCK, "seq"),
                "cache.read": sysbench_memory(path, "read", CACHE_BLOCK, "seq"),
            },
            detail={
                "block": CACHE_BLOCK,
                "seconds": SECONDS,
                "mode": "seq",
                "threads": int(THREADS),
                "working_set": parse_size(CACHE_BLOCK),
            },
        ),
    ]
