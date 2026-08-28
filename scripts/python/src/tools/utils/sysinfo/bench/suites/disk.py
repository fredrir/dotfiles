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
    FIO_WRITE_SIZE,
)
from tools.utils.sysinfo.bench.record import HIB, HOST, LIB
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


def global_options(size, target, ioengine):
    options = [
        f"ioengine={ioengine}",
        "direct=1",
        f"size={size}",
        f"iodepth={FIO_IODEPTH}",
        f"ramp_time={FIO_RAMP_SECONDS}",
        "group_reporting=1",
        f"directory={target}",
    ]
    if sys.platform != "darwin":
        # fio on Darwin rejects disk_util outright and drops the whole job file,
        # so both engines fail and the suite can never run.
        options.append("disk_util=0")
    return options


def job_file(size, target, ioengine, writes):
    lines = ["[global]", *global_options(size, target, ioengine), ""]
    for name, mode, block, _key in STAGES:
        lines.extend([f"[{name}]", f"rw={mode}", f"bs={block}"])
        limit = writes.get(name)
        if limit:
            # Size bounded, so the bytes this stage costs are known in advance.
            lines.append(f"io_size={limit}")
        else:
            lines.extend(["time_based=1", f"runtime={FIO_RUNTIME_SECONDS}"])
        lines.extend(["stonewall", ""])
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


def complaint(result):
    for line in reversed((result.stderr or "").splitlines()):
        if line.strip():
            return line.strip()
    return f"exited {result.returncode}"


def measure(path, size, target, writes, detail):
    spec = os.path.join(target, "bench.fio")
    refused = []
    values = None
    try:
        for ioengine in (engine(), "psync"):
            with open(spec, "w", encoding="utf-8") as handle:
                handle.write(job_file(size, target, ioengine, writes))
            result = run([path, "--output-format=json", spec], timeout=900)
            if result.returncode != 0:
                refused.append(f"{ioengine}: {complaint(result)}")
                continue
            try:
                values = parse(json.loads(result.stdout))
            except json.JSONDecodeError as error:
                raise MeasurementError("fio produced unreadable output") from error
            # Recorded after the fallback resolves, so the run says which engine
            # actually produced these numbers rather than which one was preferred.
            detail["engine"] = ioengine
            break
    finally:
        # Every exit runs this: a parse error, the 900s timeout and Ctrl-C all
        # used to leave the multi-gigabyte layout files behind for good.
        laid_out = discard_files(target, size)
        if os.path.exists(spec):
            os.unlink(spec)
    if values is None:
        raise MeasurementError(f"fio could not run any I/O engine ({'; '.join(refused)})")
    values[WRITTEN] += laid_out
    return values


def outputs():
    found = []
    for _stage, _mode, _block, key in STAGES:
        scale = "IOPS" if "rand" in key else "MB/s"
        found.append(Output(key, scale, HIB, HOST))
        found.append(Output(f"{key}_p99", "µs", LIB, HOST))
    return tuple(found)


def parse_size(value):
    units = {"k": 1024, "m": 1024**2, "g": 1024**3}
    suffix = value[-1].lower()
    if suffix in units:
        return int(float(value[:-1]) * units[suffix])
    return int(value)


def predicted_writes(size, writes):
    total = parse_size(size) * len(STAGES)
    for value in writes.values():
        total += parse_size(value)
    return total


def jobs(setting):
    size = FIO_SIZE.get(setting.tier, "")
    if not size:
        return []
    path = tool_path("fio")
    if not path:
        return []
    target = setting.workdir
    writes = FIO_WRITE_SIZE.get(setting.tier, {})
    detail = {
        "size": size,
        "runtime": FIO_RUNTIME_SECONDS,
        "ramp_time": FIO_RAMP_SECONDS,
        "iodepth": FIO_IODEPTH,
        "direct": 1,
        "engine": engine(),
        "write_size": dict(writes),
    }
    return [
        Job(
            name="disk",
            tool="fio",
            version=version_of(path, args=("--version",), pattern=r"fio-(\d[\d.]*)"),
            method="disk/2.0.0",
            outputs=outputs(),
            measure=lambda: measure(path, size, target, writes, detail),
            writes=predicted_writes(size, writes),
            repeat=False,
            detail=detail,
        )
    ]
