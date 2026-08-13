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

# Comfortably past the last-level cache of current desktop parts. A little of
# this still lands in a very large L3, so treat it as a floor on DRAM bandwidth
# rather than an exact figure -- it is consistent across machines, which is what
# a comparison needs.
DRAM_BLOCK = "256M"

# One thread cannot saturate a memory controller. On Apple Silicon a single
# scalar load loop is issue bound well below both cache and DRAM bandwidth, so
# single-threaded numbers show no cache cliff at all and understate the machine.
WORKING_SET_SHARE = 0.25

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


def threads_for(block):
    count = os.cpu_count() or 1
    total = physical_memory()
    size = parse_size(block)
    if not total or not size:
        return count
    allowed = int(total * WORKING_SET_SHARE) // size
    return max(1, min(count, allowed))


def sysbench_memory(path, operation, block, mode, threads):
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
    dram_threads = threads_for(DRAM_BLOCK)
    cache_threads = threads_for(CACHE_BLOCK)
    return [
        Job(
            name="mem.bandwidth",
            tool="sysbench",
            version=version,
            method="mem.bandwidth/2.0.0",
            outputs=(
                Output("mem.write", "MiB/s", HIB, WORLD),
                Output("mem.read", "MiB/s", HIB, WORLD),
            ),
            measure=lambda: {
                "mem.write": sysbench_memory(path, "write", DRAM_BLOCK, "seq", dram_threads),
                "mem.read": sysbench_memory(path, "read", DRAM_BLOCK, "seq", dram_threads),
            },
            detail={
                "block": DRAM_BLOCK,
                "seconds": SECONDS,
                "mode": "seq",
                "threads": dram_threads,
                "working_set": parse_size(DRAM_BLOCK) * dram_threads,
            },
        ),
        Job(
            name="mem.random",
            tool="sysbench",
            version=version,
            method="mem.random/2.0.0",
            outputs=(Output("mem.random", "MiB/s", HIB, WORLD),),
            measure=lambda: {
                "mem.random": sysbench_memory(path, "read", DRAM_BLOCK, "rnd", dram_threads),
            },
            detail={
                "block": DRAM_BLOCK,
                "seconds": SECONDS,
                "mode": "rnd",
                "threads": dram_threads,
                "working_set": parse_size(DRAM_BLOCK) * dram_threads,
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
                "cache.write": sysbench_memory(path, "write", CACHE_BLOCK, "seq", cache_threads),
                "cache.read": sysbench_memory(path, "read", CACHE_BLOCK, "seq", cache_threads),
            },
            detail={
                "block": CACHE_BLOCK,
                "seconds": SECONDS,
                "mode": "seq",
                "threads": cache_threads,
                "working_set": parse_size(CACHE_BLOCK) * cache_threads,
            },
        ),
    ]
