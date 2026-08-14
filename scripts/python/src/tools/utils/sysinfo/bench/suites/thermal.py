import json
import os
import shutil
import subprocess
import sys
import time

from tools.core.process import capture as run
from tools.utils.sysinfo.bench.limits import SUSTAINED_SECONDS
from tools.utils.sysinfo.bench.record import HIB, HOST, LIB
from tools.utils.sysinfo.bench.suites import Job, MeasurementError, Output, tool_path, version_of
from tools.utils.sysinfo.formatting import as_dict

SAMPLE_INTERVAL = 2.0


def sensor_temperature():
    if sys.platform == "darwin":
        return None
    if not shutil.which("sensors"):
        return None
    result = run(["sensors", "-j"], timeout=10)
    if result.returncode != 0:
        return None
    try:
        payload = json.loads(result.stdout)
    except ValueError:
        return None
    best = None
    for chip, readings in payload.items():
        if not any(mark in chip.lower() for mark in ("k10temp", "coretemp", "zenpower")):
            continue
        for entries in as_dict(readings).values():
            for name, value in as_dict(entries).items():
                if "input" in name and isinstance(value, (int, float)):
                    best = value if best is None else max(best, value)
    return best


def package_clock():
    try:
        with open("/proc/cpuinfo", encoding="utf-8") as handle:
            values = [
                float(line.split(":", 1)[1])
                for line in handle.read().splitlines()
                if line.lower().startswith("cpu mhz")
            ]
    except OSError:
        return None
    return sum(values) / len(values) if values else None


def complaint(text):
    for line in reversed((text or "").splitlines()):
        if line.strip():
            return line.strip()
    return ""


def sustained(path, seconds):
    workers = str(max(1, os.cpu_count() or 2))
    process = subprocess.Popen(
        [path, "--matrix", workers, "--timeout", f"{seconds}s", "--metrics-brief"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    temperatures = []
    clocks = []
    deadline = time.monotonic() + seconds
    stopped_early = False
    try:
        while time.monotonic() < deadline:
            if process.poll() is not None:
                stopped_early = True
                break
            time.sleep(SAMPLE_INTERVAL)
            reading = sensor_temperature()
            if reading is not None:
                temperatures.append(reading)
            clock = package_clock()
            if clock is not None:
                clocks.append(clock)
    finally:
        if process.poll() is None:
            process.terminate()
        try:
            _output, refused = process.communicate(timeout=30)
        except subprocess.TimeoutExpired:
            # stress-ng ignores SIGTERM on some platforms.
            process.kill()
            _output, refused = process.communicate()
    # Popen returns before the child can fail to exec, so a stressor that rejects
    # its arguments still leaves poll() as None for the first pass. Without this
    # the single idle sample taken in that pass was published as the peak
    # temperature under sustained load, with no error.
    if stopped_early and process.returncode:
        raise MeasurementError(
            f"stress-ng exited {process.returncode}: {complaint(refused)}".rstrip(": ")
        )
    if not temperatures and not clocks:
        raise MeasurementError("no thermal telemetry was available during the load")
    values = {}
    if temperatures:
        values["thermal.peak"] = max(temperatures)
        tail = temperatures[len(temperatures) // 2 :]
        values["thermal.steady"] = sum(tail) / len(tail)
    if clocks:
        tail = clocks[len(clocks) // 2 :]
        values["cpu.sustained_clock"] = sum(tail) / len(tail)
    return values


def jobs(setting):
    seconds = SUSTAINED_SECONDS.get(setting.tier, 0)
    if not seconds:
        return []
    path = tool_path("stress-ng")
    if not path:
        return []
    return [
        Job(
            name="thermal",
            tool="stress-ng",
            version=version_of(path, args=("--version",), pattern=r"(\d[\d.]*)"),
            method="thermal/1.0.0",
            # Host scoped: these depend on the cooler and the case, not the OS.
            # Under platform scope two unrelated machines running the same
            # distro had their temperatures compared and labelled better/worse.
            outputs=(
                Output("thermal.peak", "°C", LIB, HOST),
                Output("thermal.steady", "°C", LIB, HOST),
                Output("cpu.sustained_clock", "MHz", HIB, HOST),
            ),
            measure=lambda: sustained(path, seconds),
            repeat=False,
            detail={"seconds": seconds, "stressor": "matrix", "interval": SAMPLE_INTERVAL},
        )
    ]
