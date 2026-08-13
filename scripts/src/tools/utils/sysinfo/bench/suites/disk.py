import glob
import json
import os
import sys

from tools.core.process import capture as run
from tools.utils.sysinfo.bench.limits import (
    FIO_IODEPTH,
    FIO_RAMP_SECONDS,
    FIO_RUNTIME_SECONDS,
    FIO_SIZE,
)
from tools.utils.sysinfo.bench.record import HIB, LIB, PLATFORM
from tools.utils.sysinfo.bench.suites import (
    WRITTEN,
    Job,
    MeasurementError,
    Output,
    tool_path,
    version_of,
)

STAGES = (
    ("seq-read", "read", "1m", "disk.seq_read"),
    ("seq-write", "write", "1m", "disk.seq_write"),
    ("rand-read", "randread", "4k", "disk.rand_read"),
    ("rand-write", "randwrite", "4k", "disk.rand_write"),
)


def engine():
    return "posixaio" if sys.platform == "darwin" else "libaio"


def job_file(size, target, ioengine):
    lines = [
        "[global]",
        f"ioengine={ioengine}",
        "direct=1",
        "time_based=1",
        f"runtime={FIO_RUNTIME_SECONDS}",
        f"ramp_time={FIO_RAMP_SECONDS}",
        f"size={size}",
        f"iodepth={FIO_IODEPTH}",
        "group_reporting=1",
        "disk_util=0",
        f"directory={target}",
        "",
    ]
    for name, mode, block, _key in STAGES:
        lines.extend([f"[{name}]", f"rw={mode}", f"bs={block}", "stonewall", ""])
    return "\n".join(lines)


def parse(payload):
    values = {}
    written = 0
    for job in payload.get("jobs", []):
        name = job.get("jobname", "")
        written += job.get("write", {}).get("io_bytes") or 0
        for stage, _mode, _block, key in STAGES:
            if stage != name:
                continue
            side = job.get("write" if "write" in stage else "read", {})
            if "rand" in stage:
                values[key] = side.get("iops") or 0.0
            else:
                values[key] = (side.get("bw_bytes") or 0) / 1000000
            latency = side.get("clat_ns", {}).get("percentile", {}).get("99.000000")
            if latency:
                values[f"{key}_p99"] = latency / 1000
    if not values:
        raise MeasurementError("fio produced no usable results")
    values[WRITTEN] = float(written)
    return values


def discard_files(target, size):
    laid_out = 0
    for stage, _mode, _block, _key in STAGES:
        for path in glob.glob(os.path.join(target, f"{stage}.*")):
            try:
                laid_out += os.path.getsize(path)
                os.unlink(path)
            except OSError:
                continue
    return laid_out or parse_size(size) * len(STAGES)


def measure(path, size, target):
    spec = os.path.join(target, "bench.fio")
    try:
        for ioengine in (engine(), "psync"):
            with open(spec, "w", encoding="utf-8") as handle:
                handle.write(job_file(size, target, ioengine))
            result = run([path, "--output-format=json", spec], timeout=900)
            if result.returncode != 0:
                discard_files(target, size)
                continue
            try:
                values = parse(json.loads(result.stdout))
            except json.JSONDecodeError as error:
                raise MeasurementError("fio produced unreadable output") from error
            values[WRITTEN] += discard_files(target, size)
            return values
    finally:
        if os.path.exists(spec):
            os.unlink(spec)
    raise MeasurementError("fio could not run any I/O engine")


def outputs():
    found = []
    for _stage, _mode, _block, key in STAGES:
        scale = "IOPS" if "rand" in key else "MB/s"
        found.append(Output(key, scale, HIB, PLATFORM))
        found.append(Output(f"{key}_p99", "µs", LIB, PLATFORM))
    return tuple(found)


def jobs(setting):
    size = FIO_SIZE.get(setting.tier, "")
    if not size:
        return []
    path = tool_path("fio")
    if not path:
        return []
    target = setting.workdir
    return [
        Job(
            name="disk",
            tool="fio",
            version=version_of(path, args=("--version",), pattern=r"fio-(\d[\d.]*)"),
            method="disk/1.0.0",
            outputs=outputs(),
            measure=lambda: measure(path, size, target),
            repeat=False,
            detail={
                "size": size,
                "runtime": FIO_RUNTIME_SECONDS,
                "ramp_time": FIO_RAMP_SECONDS,
                "iodepth": FIO_IODEPTH,
                "direct": 1,
                "engine": engine(),
            },
        )
    ]


def parse_size(value):
    units = {"k": 1024, "m": 1024**2, "g": 1024**3}
    suffix = value[-1].lower()
    if suffix in units:
        return int(float(value[:-1]) * units[suffix])
    return int(value)
